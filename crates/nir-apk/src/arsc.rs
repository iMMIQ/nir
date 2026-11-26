//! Minimal pure-Rust Android `resources.arsc` builder for launcher icon packaging.
//!
//! Emits a single-package (`0x7f`), single-type (`mipmap`, id `1`), single-entry
//! (`ic_launcher`, id `0` -> resource ID `0x7f01_0000`) binary resource table
//! referencing a PNG path inside the APK (e.g. `res/mipmap-xxxhdpi/ic_launcher.png`).
//!
//! Every APK entry named `resources.arsc` must be STORED uncompressed and 4-byte
//! aligned (required on Android 11+ / API 30+).
#![forbid(unsafe_code)]

use anyhow::{ensure, Result};

/// Resource ID assigned to `@mipmap/ic_launcher` (`package=0x7f, type=0x01, entry=0x0000`).
pub const ICON_RES_ID: u32 = 0x7f01_0000;

const RES_STRING_POOL_TYPE: u16 = 0x0001;
const RES_TABLE_TYPE: u16 = 0x0002;
const RES_TABLE_PACKAGE_TYPE: u16 = 0x0200;
const RES_TABLE_TYPE_TYPE: u16 = 0x0201;
const RES_TABLE_TYPE_SPEC_TYPE: u16 = 0x0202;
const TYPE_STRING: u8 = 0x03;

/// Encodes a spec-compliant UTF-16 `ResStringPool` chunk for `strings`.
fn encode_string_pool(strings: &[&str]) -> Result<Vec<u8>> {
    let mut string_data = Vec::new();
    let mut offsets = Vec::with_capacity(strings.len());
    for s in strings {
        offsets.push(string_data.len() as u32);
        let units: Vec<u16> = s.encode_utf16().collect();
        ensure!(units.len() <= 0x7fff, "E_ARSC_STRING: string too long");
        string_data.extend_from_slice(&(units.len() as u16).to_le_bytes());
        for u in &units {
            string_data.extend_from_slice(&u.to_le_bytes());
        }
        string_data.extend_from_slice(&0u16.to_le_bytes());
        while string_data.len() % 4 != 0 {
            string_data.extend_from_slice(&0u16.to_le_bytes());
        }
    }
    let strings_start = 28u32 + 4 * strings.len() as u32;
    let total_size = strings_start + string_data.len() as u32;
    let mut out = Vec::with_capacity(total_size as usize);
    out.extend_from_slice(&RES_STRING_POOL_TYPE.to_le_bytes());
    out.extend_from_slice(&28u16.to_le_bytes());
    out.extend_from_slice(&total_size.to_le_bytes());
    out.extend_from_slice(&(strings.len() as u32).to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes()); // styleCount
    out.extend_from_slice(&0u32.to_le_bytes()); // flags (UTF-16)
    out.extend_from_slice(&strings_start.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes()); // stylesStart
    for off in &offsets {
        out.extend_from_slice(&off.to_le_bytes());
    }
    out.extend_from_slice(&string_data);
    Ok(out)
}

/// Builds a minimal valid `resources.arsc` exposing `@mipmap/ic_launcher` (`0x7f01_0000`)
/// pointing at `icon_apk_path` (e.g. `"res/mipmap-xxxhdpi/ic_launcher.png"`).
pub fn encode_launcher_icon(package_name: &str, icon_apk_path: &str) -> Result<Vec<u8>> {
    ensure!(!package_name.is_empty(), "E_ARSC_PACKAGE: empty");
    ensure!(!icon_apk_path.is_empty(), "E_ARSC_ICON: empty path");

    let global_pool = encode_string_pool(&[icon_apk_path])?;
    let type_pool = encode_string_pool(&["mipmap"])?;
    let key_pool = encode_string_pool(&["ic_launcher"])?;

    // ResTable_typeSpec (header=16, 1 entry flag = 4 bytes -> total 20 bytes)
    let mut type_spec = Vec::with_capacity(20);
    type_spec.extend_from_slice(&RES_TABLE_TYPE_SPEC_TYPE.to_le_bytes());
    type_spec.extend_from_slice(&16u16.to_le_bytes());
    type_spec.extend_from_slice(&20u32.to_le_bytes());
    type_spec.push(1u8); // type id = 1 (mipmap)
    type_spec.push(0u8); // res0
    type_spec.extend_from_slice(&0u16.to_le_bytes()); // res1
    type_spec.extend_from_slice(&1u32.to_le_bytes()); // entryCount = 1
    type_spec.extend_from_slice(&0u32.to_le_bytes()); // config flags for entry 0

    // ResTable_type (header = 20 + 64-byte default ResTable_config = 84 bytes)
    // Followed by: 1 offset (4 bytes) + ResTable_entry (8 bytes) + Res_value (8 bytes) = 104 bytes
    const CONFIG_SIZE: u32 = 64;
    const TYPE_HEADER_SIZE: u16 = 20 + CONFIG_SIZE as u16;
    const ENTRIES_START: u32 = TYPE_HEADER_SIZE as u32 + 4;
    const TYPE_TOTAL_SIZE: u32 = ENTRIES_START + 16;

    let mut type_chunk = Vec::with_capacity(TYPE_TOTAL_SIZE as usize);
    type_chunk.extend_from_slice(&RES_TABLE_TYPE_TYPE.to_le_bytes());
    type_chunk.extend_from_slice(&TYPE_HEADER_SIZE.to_le_bytes());
    type_chunk.extend_from_slice(&TYPE_TOTAL_SIZE.to_le_bytes());
    type_chunk.push(1u8); // type id = 1 (mipmap)
    type_chunk.push(0u8); // flags
    type_chunk.extend_from_slice(&0u16.to_le_bytes()); // reserved
    type_chunk.extend_from_slice(&1u32.to_le_bytes()); // entryCount = 1
    type_chunk.extend_from_slice(&ENTRIES_START.to_le_bytes());
    // Default ResTable_config (64 bytes, first u32 = size = 64, remaining bytes = 0)
    type_chunk.extend_from_slice(&CONFIG_SIZE.to_le_bytes());
    type_chunk.resize(type_chunk.len() + (CONFIG_SIZE as usize - 4), 0);
    // Entry 0 offset = 0
    type_chunk.extend_from_slice(&0u32.to_le_bytes());
    // ResTable_entry (8 bytes): size=8, flags=0, key.index=0 ("ic_launcher")
    type_chunk.extend_from_slice(&8u16.to_le_bytes());
    type_chunk.extend_from_slice(&0u16.to_le_bytes());
    type_chunk.extend_from_slice(&0u32.to_le_bytes());
    // Res_value (8 bytes): size=8, res0=0, dataType=TYPE_STRING, data=0 (global pool[0])
    type_chunk.extend_from_slice(&8u16.to_le_bytes());
    type_chunk.push(0u8);
    type_chunk.push(TYPE_STRING);
    type_chunk.extend_from_slice(&0u32.to_le_bytes());
    debug_assert_eq!(type_chunk.len(), TYPE_TOTAL_SIZE as usize);

    // ResTable_package (header = 288 bytes)
    const PKG_HEADER_SIZE: u16 = 288;
    let type_strings_off = PKG_HEADER_SIZE as u32;
    let key_strings_off = type_strings_off + type_pool.len() as u32;
    let pkg_total_size =
        key_strings_off + key_pool.len() as u32 + type_spec.len() as u32 + type_chunk.len() as u32;

    let mut pkg_chunk = Vec::with_capacity(pkg_total_size as usize);
    pkg_chunk.extend_from_slice(&RES_TABLE_PACKAGE_TYPE.to_le_bytes());
    pkg_chunk.extend_from_slice(&PKG_HEADER_SIZE.to_le_bytes());
    pkg_chunk.extend_from_slice(&pkg_total_size.to_le_bytes());
    pkg_chunk.extend_from_slice(&0x7fu32.to_le_bytes()); // package id = 0x7f
    let mut name_utf16 = [0u16; 128];
    for (dst, unit) in name_utf16
        .iter_mut()
        .take(127)
        .zip(package_name.encode_utf16())
    {
        *dst = unit;
    }
    for unit in name_utf16 {
        pkg_chunk.extend_from_slice(&unit.to_le_bytes());
    }
    pkg_chunk.extend_from_slice(&type_strings_off.to_le_bytes());
    pkg_chunk.extend_from_slice(&1u32.to_le_bytes()); // lastPublicType
    pkg_chunk.extend_from_slice(&key_strings_off.to_le_bytes());
    pkg_chunk.extend_from_slice(&1u32.to_le_bytes()); // lastPublicKey
    pkg_chunk.extend_from_slice(&0u32.to_le_bytes()); // typeIdOffset
    debug_assert_eq!(pkg_chunk.len(), PKG_HEADER_SIZE as usize);
    pkg_chunk.extend_from_slice(&type_pool);
    pkg_chunk.extend_from_slice(&key_pool);
    pkg_chunk.extend_from_slice(&type_spec);
    pkg_chunk.extend_from_slice(&type_chunk);
    debug_assert_eq!(pkg_chunk.len(), pkg_total_size as usize);

    // ResTable_header (12 bytes)
    let total_size = 12u32 + global_pool.len() as u32 + pkg_chunk.len() as u32;
    let mut out = Vec::with_capacity(total_size as usize);
    out.extend_from_slice(&RES_TABLE_TYPE.to_le_bytes());
    out.extend_from_slice(&12u16.to_le_bytes());
    out.extend_from_slice(&total_size.to_le_bytes());
    out.extend_from_slice(&1u32.to_le_bytes()); // packageCount = 1
    out.extend_from_slice(&global_pool);
    out.extend_from_slice(&pkg_chunk);
    debug_assert_eq!(out.len(), total_size as usize);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn launcher_icon_arsc_has_valid_chunk_framing() {
        let bytes = encode_launcher_icon("org.nir.demo", "res/mipmap-xxxhdpi/ic_launcher.png")
            .expect("encode arsc");
        let chunk_type = u16::from_le_bytes([bytes[0], bytes[1]]);
        let header_size = u16::from_le_bytes([bytes[2], bytes[3]]);
        let total_size = u32::from_le_bytes(bytes[4..8].try_into().unwrap()) as usize;
        let pkg_count = u32::from_le_bytes(bytes[8..12].try_into().unwrap());
        assert_eq!(chunk_type, RES_TABLE_TYPE);
        assert_eq!(header_size, 12);
        assert_eq!(total_size, bytes.len());
        assert_eq!(pkg_count, 1);

        // Verify global string pool immediately follows table header.
        let pool_type = u16::from_le_bytes([bytes[12], bytes[13]]);
        let pool_size = u32::from_le_bytes(bytes[16..20].try_into().unwrap()) as usize;
        assert_eq!(pool_type, RES_STRING_POOL_TYPE);

        // Verify package chunk follows global string pool and has id 0x7f.
        let pkg_off = 12 + pool_size;
        let pkg_type = u16::from_le_bytes([bytes[pkg_off], bytes[pkg_off + 1]]);
        let pkg_hdr = u16::from_le_bytes([bytes[pkg_off + 2], bytes[pkg_off + 3]]);
        let pkg_size =
            u32::from_le_bytes(bytes[pkg_off + 4..pkg_off + 8].try_into().unwrap()) as usize;
        let pkg_id = u32::from_le_bytes(bytes[pkg_off + 8..pkg_off + 12].try_into().unwrap());
        assert_eq!(pkg_type, RES_TABLE_PACKAGE_TYPE);
        assert_eq!(pkg_hdr, 288);
        assert_eq!(pkg_off + pkg_size, bytes.len());
        assert_eq!(pkg_id, 0x7f);
    }
}
