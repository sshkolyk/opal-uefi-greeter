use uefi::prelude::{Boot, SystemTable};
use uefi::{CStr16, Status};
use alloc::string::String;
use alloc::vec::Vec;
use core::fmt::Write;
use uefi::proto::console::text::{Color, Key, ScanCode};
use uefi::table::boot::{EventType, TimerTrigger, Tpl};
use uefi::table::runtime::ResetType;
use crate::{info, error};
use crate::error::ResultFixupExt;
use zeroize::Zeroizing;

/// A character that lands within this window of the previous one, and is that
/// key's unshifted twin, is flagged as a possible Shift-release phantom. In
/// 100 ns units (200 ms) -- deliberately generous, since nothing is dropped
/// automatically.
const PHANTOM_WINDOW_100NS: u64 = 2_000_000;

const HINT: &str = "[!] a keystroke looks doubled  -  F1: show password   F2: delete flagged";

pub fn config_stdout(st: &mut SystemTable<Boot>) -> uefi::Result {
    st.stdout().reset(false)?.log();

    if let Some(mode) = st.stdout().modes().max_by_key(|m| {
        let m = m.log();
        if m.columns() > 120 {
            0
        } else {
            m.rows() * m.columns()
        }
    }) {
        st.stdout().set_mode(mode.split().1)?.log();
    };
    Ok(().into())
}

pub fn write_char(st: &mut SystemTable<Boot>, ch: u16) -> error::Result {
    let str = &[ch, 0];
    st.stdout()
        .output_string(unsafe { CStr16::from_u16_with_nul_unchecked(str) })
        .fix(info!())
}

pub fn read_password(st: &mut SystemTable<Boot>, prompt: &str) -> error::Result<Zeroizing<String>> {
    st.stdout().write_str(prompt).unwrap();

    let mut wait_events = [unsafe { st.stdin().wait_for_key_event().unsafe_clone() }];
    let mut password = Zeroizing::new(String::with_capacity(32));
    let mut masked = true;

    // Non-destructive phantom detection: `suspects` holds char indices that look
    // like Shift-release phantoms; the character stays in the password and the
    // user decides (F1 to check, F2 to drop the last flagged one).
    let mut suspects: Vec<usize> = Vec::new();
    let mut prev: Option<char> = None;
    let mut hinted = false;
    // width of the password area currently drawn on screen
    let mut shown: usize = 0;

    let timer = unsafe {
        st.boot_services()
            .create_event(EventType::TIMER, Tpl::APPLICATION, None, None)
    }
    .fix(info!())?;

    loop {
        st.boot_services()
            .wait_for_event(&mut wait_events)
            .fix(info!())?;

        while let Some(key) = st.stdin().read_key().fix(info!())? {
            match key {
                // enter
                Key::Printable(ch) if matches!(u16::from(ch), 0x0D | 0x0A) => {
                    newline(st)?;
                    let _ = st
                        .boot_services()
                        .close_event(unsafe { timer.unsafe_clone() });
                    return Ok(password);
                }
                // backspace
                Key::Printable(ch) if u16::from(ch) == 0x08 => {
                    if !password.is_empty() {
                        let last = password.chars().count() - 1;
                        password.pop();
                        suspects.retain(|&s| s != last);
                        prev = password.chars().last();
                        rerender(st, &mut shown, &password, masked, &suspects)?;
                    }
                }
                // ignore spurious null keystrokes (scan_code == 0 && unicode_char == 0)
                Key::Printable(ch) if u16::from(ch) == 0x00 => {}
                // other printable characters
                Key::Printable(ch) => {
                    let c: char = ch.into();
                    // did the inter-key timer already fire? (not fired => within window)
                    let within = st
                        .boot_services()
                        .check_event(unsafe { timer.unsafe_clone() })
                        .map(|c| !c.log())
                        .unwrap_or(false);
                    let suspect = within
                        && matches!(prev, Some(p) if unshifted_twin(p) == Some(c));

                    let idx = password.chars().count();
                    password.push(c);
                    if suspect {
                        suspects.push(idx);
                    }

                    if suspect && !hinted {
                        hinted = true;
                        newline(st)?;
                        let _ = st.stdout().set_color(Color::Yellow, Color::Black);
                        st.stdout().write_str(HINT).unwrap();
                        let _ = st.stdout().set_color(Color::LightGray, Color::Black);
                        newline(st)?;
                        st.stdout().write_str(prompt).unwrap();
                        shown = 0;
                        rerender(st, &mut shown, &password, masked, &suspects)?;
                    } else if suspect {
                        rerender(st, &mut shown, &password, masked, &suspects)?;
                    } else {
                        write_char(st, if masked { '*' as u16 } else { c as u16 })?;
                        shown += 1;
                    }

                    prev = Some(c);
                    st.boot_services()
                        .set_timer(&timer, TimerTrigger::Relative(PHANTOM_WINDOW_100NS))
                        .fix(info!())?;
                }
                // F1 toggles showing the typed password instead of asterisks
                Key::Special(ScanCode::FUNCTION_1) => {
                    masked = !masked;
                    rerender(st, &mut shown, &password, masked, &suspects)?;
                }
                // F2 drops the last flagged character
                Key::Special(ScanCode::FUNCTION_2) => {
                    if let Some(idx) = suspects.pop() {
                        if let Some((byte, _)) = password.char_indices().nth(idx) {
                            password.remove(byte);
                        }
                        for s in suspects.iter_mut() {
                            if *s > idx {
                                *s -= 1;
                            }
                        }
                        prev = password.chars().last();
                        rerender(st, &mut shown, &password, masked, &suspects)?;
                    }
                }
                // shutdown on escape
                Key::Special(ScanCode::ESCAPE) => {
                    st.runtime_services()
                        .reset(ResetType::Shutdown, Status::SUCCESS, None);
                }

                _ => {}
            }
        }
    }
}

/// US-QWERTY unshifted twin of a shifted character, e.g. `}` -> `]`. Covers
/// punctuation, digit-row and letters -- detection is only a hint, so it can be
/// broad.
fn unshifted_twin(shifted: char) -> Option<char> {
    Some(match shifted {
        '~' => '`',
        '!' => '1',
        '@' => '2',
        '#' => '3',
        '$' => '4',
        '%' => '5',
        '^' => '6',
        '&' => '7',
        '*' => '8',
        '(' => '9',
        ')' => '0',
        '_' => '-',
        '+' => '=',
        '{' => '[',
        '}' => ']',
        '|' => '\\',
        ':' => ';',
        '"' => '\'',
        '<' => ',',
        '>' => '.',
        '?' => '/',
        'A'..='Z' => shifted.to_ascii_lowercase(),
        _ => return None,
    })
}

fn newline(st: &mut SystemTable<Boot>) -> error::Result<()> {
    write_char(st, 0x0D)?;
    write_char(st, 0x0A)
}

/// Erase the `*shown` cells of the current password area and redraw it: one
/// `*` per character when masked, the characters themselves when revealed, with
/// flagged ones drawn in red so a phantom stands out.
fn rerender(
    st: &mut SystemTable<Boot>,
    shown: &mut usize,
    password: &str,
    masked: bool,
    suspects: &[usize],
) -> error::Result<()> {
    for _ in 0..*shown {
        write_char(st, 0x08)?;
    }
    for _ in 0..*shown {
        write_char(st, ' ' as u16)?;
    }
    for _ in 0..*shown {
        write_char(st, 0x08)?;
    }

    for (i, ch) in password.chars().enumerate() {
        let flagged = !masked && suspects.contains(&i);
        if flagged {
            let _ = st.stdout().set_color(Color::LightRed, Color::Black);
        }
        write_char(st, if masked { '*' as u16 } else { ch as u16 })?;
        if flagged {
            let _ = st.stdout().set_color(Color::LightGray, Color::Black);
        }
    }
    *shown = password.chars().count();
    Ok(())
}
