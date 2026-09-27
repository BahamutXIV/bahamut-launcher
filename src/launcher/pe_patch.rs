//! PE patch planner: compute the byte payloads that the native
//! Windows path and the Wine path write into `ffxivgame.exe`.
//!
//! Two implemented contract patches live here, both tracked with their
//! retail behavior limits in `docs/handshake.md`:
//!
//! - `ENCRYPTION_TIME` at RVA `0x9A15E3`, 5 bytes: pins the client's
//!   server-UTC read to the retired-era timestamp the launcher contract
//!   assumes.
//! - `LOBBY_HOST_NAME` at RVA `0xB90110`, `0x14` bytes: NUL-terminated
//!   lobby host name; total bytes including the NUL must fit the slot.
//!
//! The three Wine stability patches (assert-log forwarder, null-this
//! guard, null-member8 NOP) are planned here too, but they are
//! behaviour changes against the client image rather than part of the
//! cross-platform handshake contract, so only the Wine backends use them.
//!
//! This module is strictly a planner: it produces `(rva, bytes)` pairs
//! and validates their inputs. The write paths live under
//! `src/platform/`: `WriteProcessMemory` on Windows, a working-copy file
//! edit for the plain Wine launch, and the x86 loader's in-memory writes
//! for the Wine extension launch.

/// RVA of the 5-byte server-UTC immediate-load patch.
pub const ENCRYPTION_TIME_PATCH_RVA: u32 = 0x009A_15E3;

/// Payload `mov eax, 0x50E0E812` for the client's server-UTC path.
///
/// Cross-reference: the encoded launch argument plaintext uses the
/// decimal value `1_356_916_742` (see
/// [`crate::launcher::launch_args::SERVER_UTC_PLAINTEXT`]) while these
/// patch bytes decode to little-endian `0x50E0E812` = `1_356_916_754`.
/// The two differ by 12 seconds and are kept as separate facts per
/// `docs/handshake.md`; do not collapse them without a
/// client-binary recheck.
pub const ENCRYPTION_TIME_PATCH_BYTES: [u8; 5] = [0xB8, 0x12, 0xE8, 0xE0, 0x50];

/// RVA of the lobby host name slot.
pub const LOBBY_HOST_NAME_RVA: u32 = 0x00B9_0110;

/// Total size of the lobby host name slot, including the NUL terminator.
pub const LOBBY_HOST_NAME_SLOT_SIZE: usize = 0x14;

/// One patch with an RVA relative to the PE image base and exact bytes to write.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PePatch {
    pub rva: u32,
    pub bytes: Vec<u8>,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum PePatchError {
    #[error("lobby host name is {actual} bytes including NUL; slot is {slot} bytes")]
    LobbyHostTooLong { actual: usize, slot: usize },
    #[error("lobby host name contains an interior NUL byte")]
    LobbyHostHasInteriorNul,
}

pub fn encryption_time_patch() -> PePatch {
    PePatch {
        rva: ENCRYPTION_TIME_PATCH_RVA,
        bytes: ENCRYPTION_TIME_PATCH_BYTES.to_vec(),
    }
}

/// Build a NUL-terminated lobby-host patch; interior NULs and inputs over [`LOBBY_HOST_NAME_SLOT_SIZE`] are rejected, but empty input is accepted.
pub fn lobby_host_patch(host: &str) -> Result<PePatch, PePatchError> {
    let host_bytes = host.as_bytes();
    if host_bytes.contains(&0) {
        return Err(PePatchError::LobbyHostHasInteriorNul);
    }
    let total = host_bytes.len() + 1;
    if total > LOBBY_HOST_NAME_SLOT_SIZE {
        return Err(PePatchError::LobbyHostTooLong {
            actual: total,
            slot: LOBBY_HOST_NAME_SLOT_SIZE,
        });
    }
    let mut bytes = Vec::with_capacity(total);
    bytes.extend_from_slice(host_bytes);
    bytes.push(0);
    Ok(PePatch {
        rva: LOBBY_HOST_NAME_RVA,
        bytes,
    })
}

/// Plan the contract patches in encryption-time-then-lobby-host order; the order is conventional, not contractual.
pub fn plan_contract_patches(lobby_host: &str) -> Result<[PePatch; 2], PePatchError> {
    let encryption = encryption_time_patch();
    let host = lobby_host_patch(lobby_host)?;
    Ok([encryption, host])
}

// ---------------------------------------------------------------------------
// Wine-path stability patches.
//
// These three are NOT part of the cross-platform handshake contract - they are
// behaviour changes against the client image that keep FFXIV 1.0 stable under
// Wine/WineD3D, so they are applied only on the Wine backends (never on the
// native Windows path, which runs against real Direct3D 9). The RVAs and byte
// payloads are properties of the 1.23b client image.
// ---------------------------------------------------------------------------

/// RVA of the indirect `call *[..] ; movl $0, [0]` block in the client's fatal-assert handler.
pub const ASSERT_LOG_PATCH_RVA: u32 = 0x0064_8BBF;

/// RVA of the byte-getter helper that dereferences a NULL `__thiscall` `this` under Wine.
pub const NULL_THIS_GUARD_PATCH_RVA: u32 = 0x0049_2550;

/// RVA of the follow-on `mov BYTE PTR [esi+0x20], 0` that faults on a NULL member.
pub const NULL_MEMBER8_WRITE_NOP_RVA: u32 = 0x0049_4B70;

/// Forwards the assert handler to `OutputDebugStringA` at VMA `0x00F3E164`, repays the stdcall deficit, and logs instead of crashing; pair with `WINEDEBUG=...,+debugstr`.
pub const ASSERT_LOG_PATCH_BYTES: [u8; 16] = [
    0xFF, 0x15, 0x64, 0xE1, 0xF3, 0x00, // call dword ptr [0x00F3E164]
    0x8D, 0x64, 0x24, 0xFC, // lea esp, [esp-4]  (repay stdcall's 4-byte pop)
    0x90, 0x90, 0x90, 0x90, 0x90, 0x90, // nop * 6
];

/// Guards NULL `this` with a zero-byte return while preserving the original non-NULL path; fits in the 32 bytes of headroom before the next function.
pub const NULL_THIS_GUARD_PATCH_BYTES: [u8; 29] = [
    0x85, 0xC9, // test ecx, ecx
    0x74, 0x0E, // je +0x0E
    0x8B, 0x49, 0x04, // mov ecx, [ecx+4]   (original first instruction)
    0x8B, 0x01, // mov eax, [ecx]
    0x8B, 0x50, 0x14, // mov edx, [eax+14h]
    0xFF, 0xD2, // call edx
    0x8A, 0x08, // mov cl, [eax]
    0xEB, 0x02, // jmp +2 (skip xor)
    0x30, 0xC9, // xor cl, cl   (default byte for the null-this path)
    0x8B, 0x44, 0x24, 0x04, // mov eax, [esp+4]
    0x88, 0x08, // mov [eax], cl
    0xC2, 0x04, 0x00, // ret 4
];

/// NOPs the 4-byte `mov BYTE PTR [esi+0x20], 0`; the SEH-protected reset retries when the member is valid.
pub const NULL_MEMBER8_WRITE_NOP_BYTES: [u8; 4] = [0x90, 0x90, 0x90, 0x90];

pub fn assert_log_patch() -> PePatch {
    PePatch {
        rva: ASSERT_LOG_PATCH_RVA,
        bytes: ASSERT_LOG_PATCH_BYTES.to_vec(),
    }
}

pub fn null_this_guard_patch() -> PePatch {
    PePatch {
        rva: NULL_THIS_GUARD_PATCH_RVA,
        bytes: NULL_THIS_GUARD_PATCH_BYTES.to_vec(),
    }
}

pub fn null_member8_write_nop_patch() -> PePatch {
    PePatch {
        rva: NULL_MEMBER8_WRITE_NOP_RVA,
        bytes: NULL_MEMBER8_WRITE_NOP_BYTES.to_vec(),
    }
}

/// Plan the three Wine/WineD3D stability patches in application order; never used by the native Windows path.
pub fn plan_wine_stability_patches() -> [PePatch; 3] {
    [
        assert_log_patch(),
        null_this_guard_patch(),
        null_member8_write_nop_patch(),
    ]
}

/// Plan the two contract patches followed by the three Wine stability patches for the working-copy launch.
pub fn plan_wine_patches(lobby_host: &str) -> Result<Vec<PePatch>, PePatchError> {
    let mut patches = Vec::from(plan_contract_patches(lobby_host)?);
    patches.extend(plan_wine_stability_patches());
    Ok(patches)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encryption_time_patch_has_fixed_shape() {
        let patch = encryption_time_patch();
        assert_eq!(patch.rva, ENCRYPTION_TIME_PATCH_RVA);
        assert_eq!(patch.bytes.len(), 5);
        assert_eq!(patch.bytes, ENCRYPTION_TIME_PATCH_BYTES);
    }

    #[test]
    fn encryption_time_patch_bytes_decode_to_expected_immediate() {
        let patch = encryption_time_patch();
        assert_eq!(patch.bytes[0], 0xB8);
        let imm = u32::from_le_bytes([
            patch.bytes[1],
            patch.bytes[2],
            patch.bytes[3],
            patch.bytes[4],
        ]);
        assert_eq!(imm, 0x50E0_E812);
    }

    #[test]
    fn lobby_host_patch_happy_path() {
        let patch = lobby_host_patch("127.0.0.1").unwrap();
        assert_eq!(patch.rva, LOBBY_HOST_NAME_RVA);
        assert_eq!(patch.bytes.len(), b"127.0.0.1".len() + 1);
        assert_eq!(*patch.bytes.last().unwrap(), 0);
        assert_eq!(&patch.bytes[..patch.bytes.len() - 1], b"127.0.0.1");
    }

    #[test]
    fn lobby_host_patch_accepts_max_length_input() {
        let host = "a".repeat(LOBBY_HOST_NAME_SLOT_SIZE - 1);
        let patch = lobby_host_patch(&host).unwrap();
        assert_eq!(patch.bytes.len(), LOBBY_HOST_NAME_SLOT_SIZE);
        assert_eq!(*patch.bytes.last().unwrap(), 0);
    }

    #[test]
    fn lobby_host_patch_rejects_just_over_slot() {
        let host = "a".repeat(LOBBY_HOST_NAME_SLOT_SIZE);
        let err = lobby_host_patch(&host).unwrap_err();
        assert_eq!(
            err,
            PePatchError::LobbyHostTooLong {
                actual: LOBBY_HOST_NAME_SLOT_SIZE + 1,
                slot: LOBBY_HOST_NAME_SLOT_SIZE,
            }
        );
    }

    #[test]
    fn lobby_host_patch_accepts_empty_string() {
        let patch = lobby_host_patch("").unwrap();
        assert_eq!(patch.bytes, vec![0]);
    }

    #[test]
    fn lobby_host_patch_rejects_interior_nul() {
        let err = lobby_host_patch("foo\0bar").unwrap_err();
        assert_eq!(err, PePatchError::LobbyHostHasInteriorNul);
    }

    #[test]
    fn plan_contract_patches_returns_both_in_order() {
        let patches = plan_contract_patches("127.0.0.1").unwrap();
        assert_eq!(patches[0].rva, ENCRYPTION_TIME_PATCH_RVA);
        assert_eq!(patches[1].rva, LOBBY_HOST_NAME_RVA);
        assert_eq!(patches[0].bytes, ENCRYPTION_TIME_PATCH_BYTES);
        assert_eq!(*patches[1].bytes.last().unwrap(), 0);
    }

    #[test]
    fn plan_contract_patches_propagates_host_errors() {
        let err = plan_contract_patches("a".repeat(50).as_str()).unwrap_err();
        assert!(matches!(err, PePatchError::LobbyHostTooLong { .. }));
    }

    #[test]
    fn stability_patches_keep_their_rvas_and_bytes_in_order() {
        let patches = plan_wine_stability_patches();
        assert_eq!(patches[0].rva, ASSERT_LOG_PATCH_RVA);
        assert_eq!(patches[0].bytes, ASSERT_LOG_PATCH_BYTES);
        assert_eq!(patches[1].rva, NULL_THIS_GUARD_PATCH_RVA);
        assert_eq!(patches[1].bytes, NULL_THIS_GUARD_PATCH_BYTES);
        assert_eq!(patches[2].rva, NULL_MEMBER8_WRITE_NOP_RVA);
        assert_eq!(patches[2].bytes, NULL_MEMBER8_WRITE_NOP_BYTES);
    }

    #[test]
    fn wine_patches_are_contract_then_stability() {
        let wine = plan_wine_patches("127.0.0.1").unwrap();
        let mut expected = Vec::from(plan_contract_patches("127.0.0.1").unwrap());
        expected.extend(plan_wine_stability_patches());
        assert_eq!(wine, expected);
        let rvas: Vec<u32> = wine.iter().map(|patch| patch.rva).collect();
        assert_eq!(
            rvas,
            [
                ENCRYPTION_TIME_PATCH_RVA,
                LOBBY_HOST_NAME_RVA,
                ASSERT_LOG_PATCH_RVA,
                NULL_THIS_GUARD_PATCH_RVA,
                NULL_MEMBER8_WRITE_NOP_RVA,
            ]
        );
    }

    #[test]
    fn wine_patches_propagate_host_errors() {
        assert!(matches!(
            plan_wine_patches("a".repeat(50).as_str()),
            Err(PePatchError::LobbyHostTooLong { .. })
        ));
    }
}
