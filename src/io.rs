use uefi::prelude::{Boot, SystemTable};
use uefi::{CStr16, Status};
use alloc::string::String;
use core::fmt::Write;
use uefi::proto::console::text::{Key, ScanCode};
use uefi::table::runtime::ResetType;
use crate::{info, error};
use crate::error::ResultFixupExt;

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

pub fn read_password(st: &mut SystemTable<Boot>, prompt: &str) -> error::Result<String> {
    st.stdout().write_str(prompt).unwrap();

    let mut wait_events = [unsafe { st.stdin().wait_for_key_event().unsafe_clone() }];
    let mut password = String::with_capacity(32);

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
                // other printable characters
                Key::Printable(ch) => {
                    write_char(st, '*' as u16)?;
                    password.push(ch.into());
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