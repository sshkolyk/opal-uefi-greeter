use uefi::prelude::{Boot, SystemTable};
use uefi::{Guid, Handle};
use uefi::proto::media::partition::{GptPartitionType, PartitionInfo};
use alloc::string::String;
use alloc::vec::Vec;
use crate::{error, info};
use crate::error::{Error, ResultFixupExt};


fn parse_guid(s: &str) -> core::result::Result<Guid, String> {
    let parts: Vec<&str> = s.split('-').collect();
    if parts.len() != 5 {
        return Err(format!("{}:\n GUID must have 5 parts", s));
    }

    let time_low = u32::from_str_radix(parts[0], 16).map_err(|_| "invalid time_low")?;
    let time_mid = u16::from_str_radix(parts[1], 16).map_err(|_| "invalid time_mid")?;
    let time_high_and_version = u16::from_str_radix(parts[2], 16).map_err(|_| "invalid time_high")?;

    // clock_seq_and_variant
    let clock_seq_and_variant = u16::from_str_radix(parts[3], 16).map_err(|_| "invalid clock_seq")?;

    let node = u64::from_str_radix(parts[4], 16).map_err(|_| "invalid node")?;

    Ok(Guid::from_values(
        time_low,
        time_mid,
        time_high_and_version,
        clock_seq_and_variant,
        node,
    ))
}

fn find_first_boot_partition(st: &mut SystemTable<Boot>) -> error::Result<Handle> {
    let mut res = None;
    for handle in st
        .boot_services()
        .find_handles::<PartitionInfo>()
        .fix(info!())?
    {
        let pi = st
            .boot_services()
            .handle_protocol::<PartitionInfo>(handle)
            .fix(info!())?;
        let pi = unsafe { &mut *pi.get() };

        let Some(gpt) = pi.gpt_partition_entry() else {
            continue
        };

        let guid = gpt.partition_type_guid;
        if guid != GptPartitionType::EFI_SYSTEM_PARTITION {
            continue
        };

        res.replace(handle);
        return res.ok_or(Error::InvalidBootPartition);
    }
    res.ok_or(Error::NoBootPartitions)
}

fn find_boot_partition_by_guid (st: &mut SystemTable<Boot>, target_esp_guid: &Guid) -> error::Result<Handle> {
    st.boot_services()
        .find_handles::<PartitionInfo>()
        .fix(info!())?
        .into_iter()
        .find_map(|handle| {
            let pi = st
                .boot_services()
                .handle_protocol::<PartitionInfo>(handle)
                .fix(info!())
                .ok()?; // пропускаем handle если не удалось

            let pi = unsafe { &*pi.get() }; // безопасная ссылка на packed
            let guid = pi.gpt_partition_entry()?.unique_partition_guid; // копия GUID

            if guid == *target_esp_guid {
                Some(handle)
            } else {
                None
            }
        })
        .ok_or(Error::NoBootPartitions)
}

// entry point for finding the boot partition, either by GUID or just the first one found
pub fn find_boot_partition(st: &mut SystemTable<Boot>, part_uuid: Option<&str>) -> error::Result<Handle> {
    if part_uuid.is_none() || part_uuid.unwrap().is_empty() {
        return find_first_boot_partition(st);
    }
    return find_boot_partition_by_guid(st, &parse_guid(part_uuid.unwrap()).unwrap());
}
