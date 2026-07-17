#![no_std]
#![no_main]
#![feature(abi_efiapi)]
#![feature(negative_impls)]
#![feature(new_uninit)]
#![feature(maybe_uninit_slice)]
#![allow(clippy::missing_safety_doc)]

#[macro_use]
extern crate alloc;
// make sure to link this
extern crate rlibc;

use alloc::{vec::Vec};
use core::{convert::TryFrom, fmt::Write, time::Duration};
use uefi::{prelude::*, proto::{
    device_path::DevicePath,
    loaded_image::LoadedImage,
    media::{
        block::BlockIO,
        file::{File, FileAttribute, FileInfo, FileMode, FileType},
        fs::SimpleFileSystem,
    },
}, table::{boot::MemoryType, runtime::ResetType}, CString16};

use crate::{
    boot_services_ext::BootServicesExt,
    config::Config,
    error::{Error, OpalError, Result, ResultFixupExt},
    nvme_device::NvmeDevice,
    nvme_passthru::*,
    opal::{session::OpalSession, uid, LockingState, StatusCode},
    secure_device::SecureDevice,
    util::sleep,
};

pub mod boot_services_ext;
pub mod config;
pub mod dp_to_text;
pub mod error;
pub mod nvme_device;
pub mod nvme_passthru;
pub mod opal;
pub mod secure_device;
pub mod util;
pub mod boot_partition;
mod io;

#[entry]
fn main(image_handle: Handle, mut st: SystemTable<Boot>) -> Status {
    if uefi_services::init(&mut st).log_warning().is_err() {
        log::error!("Failed to initialize UEFI services");
        log::error!("Shutting down in 10s..");
        sleep(Duration::from_secs(10));
    }
    if let Err(err) = run(image_handle, &mut st) {
        log::error!("Error: {:?}", err);
        log::error!("Shutting down in 10s..");
        sleep(Duration::from_secs(10));
    }
    st.runtime_services()
        .reset(ResetType::Shutdown, Status::SUCCESS, None)
}

fn run(image_handle: Handle, st: &mut SystemTable<Boot>) -> Result {
    io::config_stdout(st).fix(info!())?;
    let config = load_config(image_handle, st)?;

    let devices = find_secure_devices(st).fix(info!())?;

    for mut device in devices {
        if device.recv_locked().fix(info!())? {
            // session mutably borrows the device
            {
                let mut prompt = config.prompt.as_deref().unwrap_or("password: ");
                let mut session = loop {
                    let password = io::read_password(st, prompt)?;

                    let mut hash = zeroize::Zeroizing::new(vec![0u8; 32]);

                    // as in sedutil-cli, maybe will change
                    pbkdf2::pbkdf2::<hmac::Hmac<sha1::Sha1>>(
                        password.as_bytes(),
                        device.proto().serial_num(),
                        75000,
                        &mut hash,
                    );

                    if let Some(s) =
                        pretty_session(st, &mut device, &*hash, config.sed_locked_msg.as_deref())?
                    {
                        break s;
                    }

                    if config.clear_on_retry {
                        st.stdout().clear().fix(info!())?;
                    }

                    prompt = config
                        .retry_prompt
                        .as_deref()
                        .unwrap_or("bad password, retry: ");
                };

                session.set_mbr_done(true)?;
                session.set_locking_range(0, LockingState::ReadWrite)?;
            }

            // reconnect the controller to see
            // the real partition pop up after unlocking
            device.reconnect_controller(st).fix(info!())?;
        }
    }

    let part_uuid = config.part_uuid.as_deref();
    let handle = boot_partition::find_boot_partition(st, part_uuid)?;

    let dp = st
        .boot_services()
        .handle_protocol::<DevicePath>(handle)
        .fix(info!())?;
    let dp = unsafe { &mut *dp.get() };

    let image = config.image;

    let buf = read_file(st, handle, &image);
    match buf {
        Err(err) if err.status() == Status::NOT_FOUND => {
            log::error!("Image '{}' not found on the boot partition", image);
            return Err(Error::ImageNotFound(image));
        }

        Err(err) => {
            log::error!("UEFI error: {:?} when loading image {}", err.status(), image);
            return Err(Error::ImageNotFound(image));
        }

        Ok(res) => {
            let buf = res.log().unwrap();
            if buf.get(0..2) != Some(&[0x4d, 0x5a]) {
                return Err(Error::ImageNotPeCoff);
            }

            let loaded_image_handle = st
                .boot_services()
                .load_image(false, image_handle, Some(dp), Some(&buf))
                .fix(info!())?;
            let loaded_image = st
                .boot_services()
                .handle_protocol::<LoadedImage>(loaded_image_handle)
                .fix(info!())?;
            let loaded_image = unsafe { &mut *loaded_image.get() };

            let args = CString16::try_from(&*config.args).or(Err(Error::ConfigArgsBadUtf16))?;
            unsafe { loaded_image.set_load_options(args.as_ptr(), args.num_bytes() as _) };

            st.boot_services()
                .start_image(loaded_image_handle)
                .fix(info!())?;

            Ok(())
        }
    }
}

fn load_config(image_handle: Handle, st: &mut SystemTable<Boot>) -> Result<Config> {
    let loaded_image = st
        .boot_services()
        .handle_protocol::<LoadedImage>(image_handle)
        .fix(info!())?;
    let device_path = st
        .boot_services()
        .handle_protocol::<DevicePath>(unsafe { &*loaded_image.get() }.device())
        .fix(info!())?;
    let device_handle = st
        .boot_services()
        .locate_device_path::<SimpleFileSystem>(unsafe { &mut *device_path.get() })
        .fix(info!())?;
    let buf = read_file(st, device_handle, "config.ini")
        .fix(info!())?
        .ok_or(Error::ConfigMissing)?;
    let config = Config::parse(&buf)?;
    log::set_max_level(config.log_level);
    log::debug!("loaded config.ini = {:#?}", config);
    Ok(config)
}

fn pretty_session<'d>(
    st: &mut SystemTable<Boot>,
    device: &'d mut SecureDevice,
    challenge: &[u8],
    sed_locked_msg: Option<&str>,
) -> Result<Option<OpalSession<'d>>> {
    match OpalSession::start(
        device,
        uid::OPAL_LOCKINGSP,
        uid::OPAL_ADMIN1,
        Some(challenge),
    ) {
        Ok(session) => Ok(Some(session)),
        Err(Error::Opal(OpalError::Status(StatusCode::NOT_AUTHORIZED))) => Ok(None),
        Err(Error::Opal(OpalError::Status(StatusCode::AUTHORITY_LOCKED_OUT))) => {
            st.stdout()
                .write_str(
                    sed_locked_msg
                        .unwrap_or("Too many bad tries, SED locked out, resetting in 10s.."),
                )
                .unwrap();
            sleep(Duration::from_secs(10));
            st.runtime_services()
                .reset(ResetType::Cold, Status::WARN_RESET_REQUIRED, None);
        }
        e => e.map(Some),
    }
}

fn find_secure_devices(st: &mut SystemTable<Boot>) -> uefi::Result<Vec<SecureDevice>> {
    let mut result = Vec::new();

    let handles = st.boot_services().find_handles::<BlockIO>()?.log();

    for handle in handles {
        let block_io = st.boot_services().handle_protocol::<BlockIO>(handle)?.log();
        let block_io = unsafe { &mut *block_io.get() };

        if block_io.media().is_logical_partition() {
            continue;
        }

        // DevicePath OPTIONAL
        let device_path = match st.boot_services().handle_protocol::<DevicePath>(handle) {
            Ok(dp) => unsafe { Some(&mut *dp.log().get()) },
            Err(_) => None,
        };

        let Some(dp) = device_path else {
            continue;
        };

        // NVMe locate
        let nvme_handle = match st
            .boot_services()
            .locate_device_path::<NvmExpressPassthru>(dp)
        {
            Ok(h) => h.log(),
            Err(_) => continue,
        };

        let nvme = st
            .boot_services()
            .handle_protocol::<NvmExpressPassthru>(nvme_handle)?.log().get();

        let device = NvmeDevice::new(nvme)?;
        let secure = SecureDevice::new(handle, device.log())?.log();

        result.push(secure);
    }

    Ok(result.into())
}

fn read_file(
    st: &mut SystemTable<Boot>,
    device: Handle,
    file: &str,
) -> uefi::Result<Option<Vec<u8>>> {
    let sfs = st
        .boot_services()
        .handle_protocol::<SimpleFileSystem>(device)?
        .log();
    let sfs = unsafe { &mut *sfs.get() };

    let file_handle = sfs
        .open_volume()?
        .log()
        .open(file, FileMode::Read, FileAttribute::empty())?
        .log();

    if let FileType::Regular(mut f) = file_handle.into_type()?.log() {
        let info = f.get_boxed_info::<FileInfo>()?.log();
        let size = info.file_size() as usize;
        let ptr = st
            .boot_services()
            .allocate_pool(MemoryType::LOADER_DATA, size)?
            .log();
        let mut buf = unsafe { Vec::from_raw_parts(ptr, size, size) };

        let read = f
            .read(&mut buf)
            .map_err(|_| uefi::Status::BUFFER_TOO_SMALL)?
            .log();
        buf.truncate(read);
        Ok(Some(buf).into())
    } else {
        Ok(None.into())
    }
}
