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

    let mut wait_for_key = [unsafe { st.stdin().wait_for_key_event().unsafe_clone() }];

    let mut data = String::with_capacity(32);
    loop {
        st.boot_services()
            .wait_for_event(&mut wait_for_key)
            .fix(info!())?;

        match st.stdin().read_key().fix(info!())? {
            Some(Key::Printable(k)) if [0xD, 0xA].contains(&u16::from(k)) => {
                write_char(st, 0x0D)?;
                write_char(st, 0x0A)?;
                break Ok(data);
            }
            Some(Key::Printable(k)) if u16::from(k) == 0x8 => {
                if data.pop().is_some() {
                    write_char(st, 0x08)?;
                }
            }
            Some(Key::Printable(k)) => {
                write_char(st, '*' as u16)?;
                data.push(k.into());
            }
            Some(Key::Special(ScanCode::ESCAPE)) => {
                st.runtime_services()
                    .reset(ResetType::Shutdown, Status::SUCCESS, None)
            }
            _ => {}
        }
    }
}