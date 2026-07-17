use uefi::prelude::{Boot, SystemTable};
use uefi::{CStr16, Status};
use alloc::string::String;
use core::fmt::Write;
use uefi::proto::console::text::{Key, ScanCode};
use uefi::table::runtime::ResetType;
use crate::{info, error};
use crate::error::ResultFixupExt;
use zeroize::Zeroizing;

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

    loop {
        st.boot_services()
            .wait_for_event(&mut wait_events)
            .fix(info!())?;

        while let Some(key) = st.stdin().read_key().fix(info!())? {
            match key {
                // enter
                Key::Printable(ch) if matches!(u16::from(ch), 0x0D | 0x0A) => {
                    newline(st)?;
                    return Ok(password);
                }
                // backspace
                Key::Printable(ch) if u16::from(ch) == 0x08 => {
                    if password.pop().is_some() {
                        backspace(st)?;
                    }
                }
                // ignore spurious null keystrokes (scan_code == 0 && unicode_char == 0)
                Key::Printable(ch) if u16::from(ch) == 0x00 => {}
                // other printable characters
                Key::Printable(ch) => {
                    write_char(st, if masked { '*' as u16 } else { u16::from(ch) })?;
                    password.push(ch.into());
                }
                // F1 toggles showing the typed password instead of asterisks
                Key::Special(ScanCode::FUNCTION_1) => {
                    masked = !masked;
                    redraw_password(st, &password, masked)?;
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

fn newline(st: &mut SystemTable<Boot>) -> error::Result<()> {
    write_char(st, 0x0D)?;
    write_char(st, 0x0A)
}

fn backspace(st: &mut SystemTable<Boot>) -> error::Result<()> {
    write_char(st, 0x08)?;
    write_char(st, ' ' as u16)?;
    write_char(st, 0x08)
}

fn redraw_password(st: &mut SystemTable<Boot>, password: &str, masked: bool) -> error::Result<()> {
    let len = password.chars().count();
    for _ in 0..len {
        write_char(st, 0x08)?;
    }
    for ch in password.chars() {
        write_char(st, if masked { '*' as u16 } else { ch as u16 })?;
    }
    Ok(())
}