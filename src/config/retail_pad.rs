//! Codec and update planner for the retail per-user `config.pad` controller selection.
//!
//! Layout: <https://github.com/XIVLegacy/xivl-decomp/blob/main/docs/input/controller-mapping.md#file-fields>.
//! Selection rules: <https://github.com/XIVLegacy/xivl-decomp/blob/main/docs/input/game-controller-startup.md>.
//! Preset 0 is `presets[0]` of <https://github.com/XIVLegacy/xivl-decomp/blob/main/config/ffxivconfig.controller_map.json>.

use std::fs::File;
use std::io::{self, Read};
use std::path::Path;

use super::atomic_write::write_atomic;
use super::retail_game::backup_path;

pub const PAD_FILE_NAME: &str = "config.pad";
pub const PAD_SIZE: usize = 0x148;
pub const PAD_MAGIC: u32 = 0x2010_0211;
pub const MAP_LEN: usize = 24;
pub const UNASSIGNED: u8 = 0xFF;
/// Upper bound on the bytes read from an existing `config.pad`.
pub const MAX_PAD_FILE_BYTES: u64 = 64 * 1024;
/// Preset 0 ("XInput Compliant Gamepad"), valid only for the XInput route.
pub const PRESET_XINPUT_COMPLIANT: [u8; MAP_LEN] = [
    34, 35, 32, 33, 38, 39, 36, 37, 42, 43, 40, 41, 9, 7, 6, 8, 4, 45, 2, 5, 47, 3, 0, 1,
];

pub type Guid = [u8; 16];

const MAGIC_OFFSET: usize = 0x00;
const INSTANCE_OFFSET: usize = 0x04;
const PRODUCT_OFFSET: usize = 0x14;
const IDENTITY_END: usize = 0x24;
const BACKGROUND_OFFSET: usize = 0x24;
const DEADZONE_OFFSET: usize = 0x28;
const MAP_OFFSET: usize = 0x2C;
const VIBRATION_OFFSET: usize = 0x44;
const DEFAULT_DEADZONE: i32 = 2;

// Product GUID bytes 4..16: Data2 and Data3 are zero, Data4 is 00 00 "PIDVID".
const PRODUCT_GUID_SUFFIX: [u8; 12] = [0, 0, 0, 0, 0, 0, b'P', b'I', b'D', b'V', b'I', b'D'];

// Steam Input's emulated Xbox 360 pad.
const PREFERRED_VENDOR: u16 = 0x045E;
const PREFERRED_PRODUCT: u16 = 0x028E;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PadIdentity {
    pub instance: Guid,
    pub product: Guid,
}

/// The product GUID the game and Wine derive from a USB vendor and product ID.
pub fn product_guid(vendor: u16, product: u16) -> Guid {
    let mut guid = [0; 16];
    guid[0..2].copy_from_slice(&vendor.to_le_bytes());
    guid[2..4].copy_from_slice(&product.to_le_bytes());
    guid[4..].copy_from_slice(&PRODUCT_GUID_SUFFIX);
    guid
}

/// The vendor and product ID of a GUID built by [`product_guid`].
pub fn product_vid_pid(guid: &Guid) -> Option<(u16, u16)> {
    if guid[4..] != PRODUCT_GUID_SUFFIX {
        return None;
    }
    Some((
        u16::from_le_bytes([guid[0], guid[1]]),
        u16::from_le_bytes([guid[2], guid[3]]),
    ))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PadSelection {
    Absent,
    Invalid {
        len: usize,
    },
    /// A valid file whose all-zero instance GUID makes the game skip selection.
    Zero,
    Selected(PadIdentity),
}

fn is_valid_image(bytes: &[u8]) -> bool {
    bytes.len() >= PAD_SIZE && read_u32(bytes, MAGIC_OFFSET) == PAD_MAGIC
}

fn read_u32(bytes: &[u8], offset: usize) -> u32 {
    let mut word = [0; 4];
    word.copy_from_slice(&bytes[offset..offset + 4]);
    u32::from_le_bytes(word)
}

fn read_guid(bytes: &[u8], offset: usize) -> Guid {
    let mut guid = [0; 16];
    guid.copy_from_slice(&bytes[offset..offset + 16]);
    guid
}

fn read_identity(bytes: &[u8]) -> PadIdentity {
    PadIdentity {
        instance: read_guid(bytes, INSTANCE_OFFSET),
        product: read_guid(bytes, PRODUCT_OFFSET),
    }
}

fn write_identity(bytes: &mut [u8], identity: &PadIdentity) {
    bytes[INSTANCE_OFFSET..PRODUCT_OFFSET].copy_from_slice(&identity.instance);
    bytes[PRODUCT_OFFSET..IDENTITY_END].copy_from_slice(&identity.product);
}

pub fn decode_selection(bytes: Option<&[u8]>) -> PadSelection {
    let Some(bytes) = bytes else {
        return PadSelection::Absent;
    };
    if !is_valid_image(bytes) {
        return PadSelection::Invalid { len: bytes.len() };
    }
    let identity = read_identity(bytes);
    if identity.instance == [0; 16] {
        PadSelection::Zero
    } else {
        PadSelection::Selected(identity)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PadFields {
    pub background_input: u32,
    pub deadzone: i32,
    pub vibration: u32,
    /// Count of map bytes other than [`UNASSIGNED`].
    pub assigned: usize,
    pub preset_xinput: bool,
}

/// Decode the scalar fields of a valid image; `None` for any other bytes.
pub fn decode_fields(bytes: &[u8]) -> Option<PadFields> {
    if !is_valid_image(bytes) {
        return None;
    }
    let map = &bytes[MAP_OFFSET..VIBRATION_OFFSET];
    Some(PadFields {
        background_input: read_u32(bytes, BACKGROUND_OFFSET),
        deadzone: read_u32(bytes, DEADZONE_OFFSET) as i32,
        vibration: read_u32(bytes, VIBRATION_OFFSET),
        assigned: map.iter().filter(|&&source| source != UNASSIGNED).count(),
        preset_xinput: map == PRESET_XINPUT_COMPLIANT.as_slice(),
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PadCandidate {
    pub vendor: u16,
    pub product: u16,
    pub index: u32,
    pub instance: Guid,
    pub xinput_route: bool,
}

impl PadCandidate {
    pub fn identity(&self) -> PadIdentity {
        PadIdentity {
            instance: self.instance,
            product: product_guid(self.vendor, self.product),
        }
    }

    fn is_preferred(&self) -> bool {
        self.vendor == PREFERRED_VENDOR && self.product == PREFERRED_PRODUCT && self.index == 0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PadWriteKind {
    Created,
    IdentityRepaired,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PadSkip {
    NoCandidates,
    Ambiguous,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PadPlan {
    Unchanged {
        matched: PadCandidate,
    },
    Write {
        image: Vec<u8>,
        chosen: PadCandidate,
        kind: PadWriteKind,
        map_written: bool,
    },
    Skip(PadSkip),
}

/// A default image selecting `chosen`, with the XInput preset only on the XInput route.
pub fn fresh_image(chosen: &PadCandidate) -> Vec<u8> {
    let mut image = vec![0; PAD_SIZE];
    image[MAGIC_OFFSET..INSTANCE_OFFSET].copy_from_slice(&PAD_MAGIC.to_le_bytes());
    write_identity(&mut image, &chosen.identity());
    image[DEADZONE_OFFSET..MAP_OFFSET].copy_from_slice(&DEFAULT_DEADZONE.to_le_bytes());
    let map = &mut image[MAP_OFFSET..VIBRATION_OFFSET];
    if chosen.xinput_route {
        map.copy_from_slice(&PRESET_XINPUT_COMPLIANT);
    } else {
        map.fill(UNASSIGNED);
    }
    image
}

fn choose(candidates: &[PadCandidate]) -> Result<&PadCandidate, PadSkip> {
    match candidates {
        [] => Err(PadSkip::NoCandidates),
        [only] => Ok(only),
        several => several
            .iter()
            .find(|candidate| candidate.is_preferred())
            .ok_or(PadSkip::Ambiguous),
    }
}

/// Decide how to bring `config.pad` in line with the enumerated controllers.
///
/// A valid file already naming an enumerated pad is left alone. Otherwise the chosen
/// candidate replaces only the identity of a valid file, or starts a fresh image.
pub fn plan_pad_update(existing: Option<&[u8]>, candidates: &[PadCandidate]) -> PadPlan {
    let valid = existing.filter(|bytes| is_valid_image(bytes));
    if let Some(bytes) = valid {
        let saved = read_identity(bytes);
        if let Some(matched) = candidates
            .iter()
            .find(|candidate| candidate.identity() == saved)
        {
            return PadPlan::Unchanged { matched: *matched };
        }
    }
    let chosen = match choose(candidates) {
        Ok(chosen) => *chosen,
        Err(skip) => return PadPlan::Skip(skip),
    };
    let Some(bytes) = valid else {
        return PadPlan::Write {
            image: fresh_image(&chosen),
            chosen,
            kind: PadWriteKind::Created,
            map_written: chosen.xinput_route,
        };
    };
    let mut image = bytes.to_vec();
    write_identity(&mut image, &chosen.identity());
    let map = &mut image[MAP_OFFSET..VIBRATION_OFFSET];
    let map_written = chosen.xinput_route && map.iter().all(|&source| source == UNASSIGNED);
    if map_written {
        map.copy_from_slice(&PRESET_XINPUT_COMPLIANT);
    }
    PadPlan::Write {
        image,
        chosen,
        kind: PadWriteKind::IdentityRepaired,
        map_written,
    }
}

/// Read a `config.pad` of at most [`MAX_PAD_FILE_BYTES`]; a missing file is `Ok(None)` and a
/// larger one is `InvalidData`.
pub fn read_pad_file(path: &Path) -> Result<Option<Vec<u8>>, io::ErrorKind> {
    let file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.kind()),
    };
    let mut bytes = Vec::new();
    file.take(MAX_PAD_FILE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| error.kind())?;
    if bytes.len() as u64 > MAX_PAD_FILE_BYTES {
        return Err(io::ErrorKind::InvalidData);
    }
    Ok(Some(bytes))
}

/// Write `image` to `path`, first saving `existing` to the `.bak` sibling.
///
/// The backup is written before `path` is touched. No directories are created.
pub fn write_planned_image(path: &Path, existing: Option<&[u8]>, image: &[u8]) -> io::Result<()> {
    if let Some(previous) = existing {
        write_atomic(&backup_path(path), previous)?;
    }
    write_atomic(path, image)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    const XBOX_PRODUCT: Guid = [
        0x5E, 0x04, 0x8E, 0x02, 0, 0, 0, 0, 0, 0, 0x50, 0x49, 0x44, 0x56, 0x49, 0x44,
    ];
    const PRESET: [u8; 24] = [
        34, 35, 32, 33, 38, 39, 36, 37, 42, 43, 40, 41, 9, 7, 6, 8, 4, 45, 2, 5, 47, 3, 0, 1,
    ];
    const TESTER_INSTANCE: Guid = [
        0xde, 0x3e, 0x57, 0x9e, 0x34, 0x77, 0xd2, 0x11, 0x8d, 0x4a, 0x23, 0x90, 0x3f, 0xb6, 0xbd,
        0xf7,
    ];
    const TESTER_PRODUCT: Guid = [
        0xde, 0x28, 0x05, 0x13, 0, 0, 0, 0, 0, 0, 0x50, 0x49, 0x44, 0x56, 0x49, 0x44,
    ];
    const TESTER_MAP: [u8; 24] = [
        34, 35, 32, 33, 38, 39, 36, 37, 42, 43, 40, 41, 9, 7, 6, 255, 4, 255, 255, 2, 255, 3, 1,
        255,
    ];
    const STEAM_INSTANCE: Guid = [
        0xd2, 0xf6, 0x48, 0xbb, 0x1b, 0xc3, 0xf1, 0x11, 0x80, 0x01, 0x44, 0x45, 0x53, 0x54, 0, 0,
    ];

    fn candidate(vendor: u16, product: u16, index: u32, xinput_route: bool) -> PadCandidate {
        PadCandidate {
            vendor,
            product,
            index,
            instance: [index as u8 + 1; 16],
            xinput_route,
        }
    }

    fn steam_pad() -> PadCandidate {
        PadCandidate {
            vendor: 0x045E,
            product: 0x028E,
            index: 0,
            instance: STEAM_INSTANCE,
            xinput_route: true,
        }
    }

    /// A literal image with distinctive non-default flags and tail bytes.
    fn image(instance: Guid, product: Guid, map: [u8; 24], total: usize) -> Vec<u8> {
        let mut bytes = vec![0u8; total];
        bytes[0..4].copy_from_slice(&[0x11, 0x02, 0x10, 0x20]);
        bytes[0x04..0x14].copy_from_slice(&instance);
        bytes[0x14..0x24].copy_from_slice(&product);
        bytes[0x24] = 1;
        bytes[0x28] = 3;
        bytes[0x2C..0x44].copy_from_slice(&map);
        bytes[0x44] = 1;
        for (offset, byte) in bytes.iter_mut().enumerate().skip(0x48) {
            *byte = 0xA0 ^ offset as u8;
        }
        bytes
    }

    fn written(plan: PadPlan) -> (Vec<u8>, PadCandidate, PadWriteKind, bool) {
        match plan {
            PadPlan::Write {
                image,
                chosen,
                kind,
                map_written,
            } => (image, chosen, kind, map_written),
            other => panic!("expected a write, got {other:?}"),
        }
    }

    #[test]
    fn product_guid_matches_the_documented_formula() {
        assert_eq!(product_guid(0x045E, 0x028E), XBOX_PRODUCT);
        assert_eq!(product_vid_pid(&XBOX_PRODUCT), Some((0x045E, 0x028E)));
        assert_eq!(product_vid_pid(&TESTER_PRODUCT), Some((0x28DE, 0x1305)));
        assert_eq!(product_vid_pid(&TESTER_INSTANCE), None);
        let mut wrong_suffix = XBOX_PRODUCT;
        wrong_suffix[15] = 0x45;
        assert_eq!(product_vid_pid(&wrong_suffix), None);
    }

    #[test]
    fn fresh_image_is_the_documented_default() {
        let bytes = fresh_image(&steam_pad());
        assert_eq!(bytes.len(), 328);
        assert_eq!(bytes[0..4], [0x11, 0x02, 0x10, 0x20]);
        assert_eq!(bytes[0x04..0x14], STEAM_INSTANCE);
        assert_eq!(bytes[0x14..0x24], XBOX_PRODUCT);
        assert_eq!(bytes[0x24..0x28], [0, 0, 0, 0]);
        assert_eq!(bytes[0x28..0x2C], [2, 0, 0, 0]);
        assert_eq!(bytes[0x2C..0x44], PRESET);
        assert_eq!(bytes[0x44..0x48], [0, 0, 0, 0]);
        assert!(bytes[0x48..].iter().all(|&byte| byte == 0));
    }

    #[test]
    fn fresh_image_leaves_the_map_unassigned_off_the_xinput_route() {
        let mut other = steam_pad();
        other.xinput_route = false;
        let bytes = fresh_image(&other);
        assert_eq!(bytes[0x2C..0x44], [0xFF; 24]);
        assert_eq!(bytes[0x28..0x2C], [2, 0, 0, 0]);
    }

    #[test]
    fn matching_both_guids_is_unchanged() {
        let pad = steam_pad();
        let existing = image(STEAM_INSTANCE, XBOX_PRODUCT, TESTER_MAP, PAD_SIZE);
        assert_eq!(
            plan_pad_update(Some(&existing), &[pad]),
            PadPlan::Unchanged { matched: pad }
        );
    }

    #[test]
    fn unchanged_wins_when_the_match_is_not_the_preferred_pad() {
        let preferred = steam_pad();
        let other = candidate(0x054C, 0x09CC, 1, false);
        let existing = image(
            other.instance,
            product_guid(0x054C, 0x09CC),
            TESTER_MAP,
            PAD_SIZE,
        );
        assert_eq!(
            plan_pad_update(Some(&existing), &[preferred, other]),
            PadPlan::Unchanged { matched: other }
        );
    }

    #[test]
    fn zero_instance_is_repaired_and_only_the_identity_changes() {
        let pad = steam_pad();
        let existing = image([0; 16], XBOX_PRODUCT, TESTER_MAP, PAD_SIZE);
        let (bytes, chosen, kind, map_written) = written(plan_pad_update(Some(&existing), &[pad]));
        let mut expected = existing.clone();
        expected[0x04..0x14].copy_from_slice(&STEAM_INSTANCE);
        assert_eq!(bytes, expected);
        assert_eq!(chosen, pad);
        assert_eq!(kind, PadWriteKind::IdentityRepaired);
        assert!(!map_written);
    }

    #[test]
    fn zero_identity_with_an_empty_map_also_gets_the_preset() {
        let existing = image([0; 16], [0; 16], [0xFF; 24], PAD_SIZE);
        let (bytes, _, kind, map_written) =
            written(plan_pad_update(Some(&existing), &[steam_pad()]));
        let mut expected = existing.clone();
        expected[0x04..0x14].copy_from_slice(&STEAM_INSTANCE);
        expected[0x14..0x24].copy_from_slice(&XBOX_PRODUCT);
        expected[0x2C..0x44].copy_from_slice(&PRESET);
        assert_eq!(bytes, expected);
        assert_eq!(kind, PadWriteKind::IdentityRepaired);
        assert!(map_written);
    }

    #[test]
    fn product_only_match_with_a_stale_instance_is_repaired() {
        let existing = image([0x77; 16], XBOX_PRODUCT, TESTER_MAP, PAD_SIZE);
        let (bytes, _, kind, _) = written(plan_pad_update(Some(&existing), &[steam_pad()]));
        assert_eq!(kind, PadWriteKind::IdentityRepaired);
        assert_eq!(bytes[0x04..0x14], STEAM_INSTANCE);
        assert_eq!(bytes[0x14..0x24], XBOX_PRODUCT);
    }

    #[test]
    fn preserved_tester_layout_survives_an_identity_repair() {
        let mut tester = vec![0u8; PAD_SIZE];
        tester[0..4].copy_from_slice(&0x2010_0211u32.to_le_bytes());
        tester[0x04..0x14].copy_from_slice(&TESTER_INSTANCE);
        tester[0x14..0x24].copy_from_slice(&TESTER_PRODUCT);
        tester[0x28] = 2;
        tester[0x2C..0x44].copy_from_slice(&TESTER_MAP);
        tester[0x44] = 1;

        let (bytes, chosen, kind, map_written) =
            written(plan_pad_update(Some(&tester), &[steam_pad()]));
        let mut expected = tester.clone();
        expected[0x04..0x14].copy_from_slice(&STEAM_INSTANCE);
        expected[0x14..0x24].copy_from_slice(&XBOX_PRODUCT);
        assert_eq!(bytes, expected);
        assert_eq!(chosen, steam_pad());
        assert_eq!(kind, PadWriteKind::IdentityRepaired);
        assert!(!map_written);
        assert_eq!(bytes[0x2C..0x44], TESTER_MAP);
        assert_eq!(bytes[0x44], 1);
    }

    #[test]
    fn the_preset_is_written_only_over_an_empty_map() {
        let mut one_set = [0xFF; 24];
        one_set[23] = 5;
        let stale = |map| image([0x77; 16], [0x66; 16], map, PAD_SIZE);

        let (bytes, _, _, map_written) =
            written(plan_pad_update(Some(&stale(one_set)), &[steam_pad()]));
        assert!(!map_written);
        assert_eq!(bytes[0x2C..0x44], one_set);

        let (bytes, _, _, map_written) =
            written(plan_pad_update(Some(&stale([0xFF; 24])), &[steam_pad()]));
        assert!(map_written);
        assert_eq!(bytes[0x2C..0x44], PRESET);
    }

    #[test]
    fn a_non_xinput_candidate_never_gets_the_map_on_repair() {
        let existing = image([0x77; 16], [0x66; 16], [0xFF; 24], PAD_SIZE);
        let other = candidate(0x054C, 0x09CC, 0, false);
        let (bytes, _, kind, map_written) = written(plan_pad_update(Some(&existing), &[other]));
        assert_eq!(kind, PadWriteKind::IdentityRepaired);
        assert!(!map_written);
        assert_eq!(bytes[0x2C..0x44], [0xFF; 24]);
    }

    #[test]
    fn short_or_foreign_files_are_replaced_by_a_fresh_image() {
        let pad = steam_pad();
        let fresh = fresh_image(&pad);
        let mut wrong_magic = image(STEAM_INSTANCE, XBOX_PRODUCT, TESTER_MAP, PAD_SIZE);
        wrong_magic[0] = 0x12;
        let short = image(STEAM_INSTANCE, XBOX_PRODUCT, TESTER_MAP, PAD_SIZE - 1);
        for existing in [vec![0x11u8; 12], wrong_magic, short] {
            let (bytes, chosen, kind, map_written) =
                written(plan_pad_update(Some(&existing), &[pad]));
            assert_eq!(bytes, fresh);
            assert_eq!(chosen, pad);
            assert_eq!(kind, PadWriteKind::Created);
            assert!(map_written);
        }
    }

    #[test]
    fn a_missing_file_is_created() {
        let other = candidate(0x054C, 0x09CC, 0, false);
        let (bytes, chosen, kind, map_written) = written(plan_pad_update(None, &[other]));
        assert_eq!(bytes, fresh_image(&other));
        assert_eq!(chosen, other);
        assert_eq!(kind, PadWriteKind::Created);
        assert!(!map_written);
    }

    #[test]
    fn a_longer_valid_file_keeps_its_extra_bytes() {
        let existing = image(TESTER_INSTANCE, TESTER_PRODUCT, TESTER_MAP, 400);
        assert_eq!(
            decode_selection(Some(&existing)),
            PadSelection::Selected(PadIdentity {
                instance: TESTER_INSTANCE,
                product: TESTER_PRODUCT,
            })
        );
        assert_eq!(decode_fields(&existing).unwrap().vibration, 1);

        let (bytes, _, kind, _) = written(plan_pad_update(Some(&existing), &[steam_pad()]));
        assert_eq!(kind, PadWriteKind::IdentityRepaired);
        assert_eq!(bytes.len(), 400);
        assert_eq!(bytes[0x148..], existing[0x148..]);
        assert_eq!(bytes[0x24..0x148], existing[0x24..0x148]);
    }

    #[test]
    fn candidate_choice_never_guesses() {
        let first = candidate(0x054C, 0x09CC, 0, false);
        let second = candidate(0x046D, 0xC21D, 1, false);
        let wrong_index = candidate(0x045E, 0x028E, 1, true);
        assert_eq!(
            plan_pad_update(None, &[first, second]),
            PadPlan::Skip(PadSkip::Ambiguous)
        );
        assert_eq!(
            plan_pad_update(None, &[first, wrong_index]),
            PadPlan::Skip(PadSkip::Ambiguous)
        );
        let (_, chosen, _, _) = written(plan_pad_update(None, &[first, steam_pad(), second]));
        assert_eq!(chosen, steam_pad());
        assert_eq!(
            plan_pad_update(None, &[]),
            PadPlan::Skip(PadSkip::NoCandidates)
        );
        let existing = image(TESTER_INSTANCE, TESTER_PRODUCT, TESTER_MAP, PAD_SIZE);
        assert_eq!(
            plan_pad_update(Some(&existing), &[]),
            PadPlan::Skip(PadSkip::NoCandidates)
        );
    }

    #[test]
    fn selection_states_are_decoded() {
        assert_eq!(decode_selection(None), PadSelection::Absent);
        assert_eq!(
            decode_selection(Some(&[0u8; 12])),
            PadSelection::Invalid { len: 12 }
        );
        let mut wrong_magic = image(TESTER_INSTANCE, TESTER_PRODUCT, TESTER_MAP, PAD_SIZE);
        wrong_magic[3] = 0x21;
        assert_eq!(
            decode_selection(Some(&wrong_magic)),
            PadSelection::Invalid { len: PAD_SIZE }
        );
        let zero = image([0; 16], TESTER_PRODUCT, TESTER_MAP, PAD_SIZE);
        assert_eq!(decode_selection(Some(&zero)), PadSelection::Zero);
        let selected = image(TESTER_INSTANCE, TESTER_PRODUCT, TESTER_MAP, PAD_SIZE);
        assert_eq!(
            decode_selection(Some(&selected)),
            PadSelection::Selected(PadIdentity {
                instance: TESTER_INSTANCE,
                product: TESTER_PRODUCT,
            })
        );
    }

    #[test]
    fn field_states_are_decoded() {
        assert_eq!(decode_fields(&[0u8; PAD_SIZE]), None);
        assert_eq!(decode_fields(&[0x11, 0x02, 0x10, 0x20]), None);

        let custom = image(TESTER_INSTANCE, TESTER_PRODUCT, TESTER_MAP, PAD_SIZE);
        assert_eq!(
            decode_fields(&custom),
            Some(PadFields {
                background_input: 1,
                deadzone: 3,
                vibration: 1,
                assigned: 19,
                preset_xinput: false,
            })
        );

        let preset = fresh_image(&steam_pad());
        assert_eq!(
            decode_fields(&preset),
            Some(PadFields {
                background_input: 0,
                deadzone: 2,
                vibration: 0,
                assigned: 24,
                preset_xinput: true,
            })
        );

        let empty = fresh_image(&candidate(0x054C, 0x09CC, 0, false));
        let fields = decode_fields(&empty).unwrap();
        assert_eq!(fields.assigned, 0);
        assert!(!fields.preset_xinput);
    }

    #[test]
    fn writing_with_an_existing_file_backs_it_up_first() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(PAD_FILE_NAME);
        let backup = dir.path().join("config.pad.bak");
        fs::write(&path, b"previous").unwrap();
        fs::write(&backup, b"older backup").unwrap();

        write_planned_image(&path, Some(b"previous"), b"replacement").unwrap();

        assert_eq!(fs::read(&path).unwrap(), b"replacement");
        assert_eq!(fs::read(&backup).unwrap(), b"previous");
    }

    #[test]
    fn a_failed_backup_leaves_the_existing_file_unchanged() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(PAD_FILE_NAME);
        fs::write(&path, b"previous").unwrap();
        fs::create_dir(dir.path().join("config.pad.bak")).unwrap();

        assert!(write_planned_image(&path, Some(b"previous"), b"replacement").is_err());

        assert_eq!(fs::read(&path).unwrap(), b"previous");
    }

    #[test]
    fn reading_a_pad_file_is_bounded() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(PAD_FILE_NAME);
        assert_eq!(read_pad_file(&path), Ok(None));
        fs::write(&path, vec![0u8; MAX_PAD_FILE_BYTES as usize]).unwrap();
        assert_eq!(
            read_pad_file(&path).unwrap().unwrap().len(),
            MAX_PAD_FILE_BYTES as usize
        );
        fs::write(&path, vec![0u8; MAX_PAD_FILE_BYTES as usize + 1]).unwrap();
        assert_eq!(read_pad_file(&path), Err(io::ErrorKind::InvalidData));
    }

    #[test]
    fn writing_without_an_existing_file_makes_no_backup() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(PAD_FILE_NAME);

        write_planned_image(&path, None, b"fresh").unwrap();

        assert_eq!(fs::read(&path).unwrap(), b"fresh");
        assert!(!dir.path().join("config.pad.bak").exists());
    }

    #[test]
    fn writing_into_a_missing_folder_fails_and_creates_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("absent");
        let path = missing.join(PAD_FILE_NAME);

        assert!(write_planned_image(&path, Some(b"previous"), b"image").is_err());
        assert!(write_planned_image(&path, None, b"image").is_err());

        assert!(!missing.exists());
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 0);
    }
}
