//! Parser for the Wine prefix `user.reg` DirectInput calibration cache.
//!
//! Wine stores one instance GUID per VID/PID and index under `DirectInput\VID_xxxx&PID_xxxx\Calibration\N`
//! ([wine-11.18 `dlls/dinput/joystick_hid.c`](https://github.com/wine-mirror/wine/blob/wine-11.18/dlls/dinput/joystick_hid.c)),
//! so the GUID is stable per prefix.

pub const USER_REG_FILE: &str = "user.reg";

const MAX_ENTRIES: usize = 64;
const GUID_LEN: usize = 16;
const KEY_PREFIX: [&str; 6] = [
    "System",
    "CurrentControlSet",
    "Control",
    "MediaProperties",
    "PrivateProperties",
    "DirectInput",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CachedPad {
    pub vendor: u16,
    pub product: u16,
    pub index: u32,
    pub instance: [u8; 16],
}

/// Cached pads in file order, at most 64. Entries without a valid 16-byte GUID are skipped.
pub fn parse_directinput_cache(user_reg: &str) -> Vec<CachedPad> {
    let mut pads = Vec::new();
    // The Calibration section being read: (vendor, product, index) and whether its GUID value was seen.
    let mut section: Option<(u16, u16, u32)> = None;
    let mut guid_seen = false;

    let mut lines = user_reg.lines();
    while let Some(line) = lines.next() {
        if line.starts_with('[') {
            section = parse_section_header(line);
            guid_seen = false;
            continue;
        }
        let Some((vendor, product, index)) = section else {
            continue;
        };
        if guid_seen || !line.starts_with('"') {
            continue;
        }
        let mut logical = line.to_string();
        while is_hex_value(&logical) && logical.ends_with('\\') {
            logical.pop();
            match lines.next() {
                Some(next) => logical.push_str(next.trim_start()),
                None => break,
            }
        }
        let Some((name, value)) = split_value(&logical) else {
            continue;
        };
        if !name.eq_ignore_ascii_case("GUID") {
            continue;
        }
        guid_seen = true;
        let Some(instance) = value.strip_prefix("hex:").and_then(parse_guid_bytes) else {
            continue;
        };
        pads.push(CachedPad {
            vendor,
            product,
            index,
            instance,
        });
        if pads.len() == MAX_ENTRIES {
            break;
        }
    }
    pads
}

fn is_hex_value(line: &str) -> bool {
    split_value(line).is_some_and(|(_, value)| value.starts_with("hex"))
}

/// Split `"name"=value` into the name and the text after the equals sign.
fn split_value(line: &str) -> Option<(&str, &str)> {
    let rest = line.strip_prefix('"')?;
    let (name, rest) = rest.split_once('"')?;
    Some((name, rest.strip_prefix('=')?))
}

fn parse_guid_bytes(text: &str) -> Option<[u8; 16]> {
    let mut bytes = [0; GUID_LEN];
    let mut items = text.split(',');
    for slot in &mut bytes {
        let item = items.next()?.trim();
        if item.len() != 2 || !item.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return None;
        }
        *slot = u8::from_str_radix(item, 16).ok()?;
    }
    items.next().is_none().then_some(bytes)
}

/// Parse `[key] timestamp` into the VID, PID and index of a DirectInput calibration key.
fn parse_section_header(line: &str) -> Option<(u16, u16, u32)> {
    let (key, tail) = line.strip_prefix('[')?.split_once(']')?;
    let tail_ok = tail.is_empty()
        || (tail.starts_with(char::is_whitespace)
            && tail.trim().bytes().all(|byte| byte.is_ascii_digit()));
    if !tail_ok {
        return None;
    }
    // The registry file doubles each backslash of the key path.
    let key = key.replace("\\\\", "\\");
    let mut segments = key.split('\\');
    for expected in KEY_PREFIX {
        if !segments.next()?.eq_ignore_ascii_case(expected) {
            return None;
        }
    }
    let device = segments.next()?;
    if !segments.next()?.eq_ignore_ascii_case("Calibration") {
        return None;
    }
    let index = segments.next()?;
    if segments.next().is_some() || index.is_empty() || !index.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let (vendor, product) = parse_device_segment(device)?;
    Some((vendor, product, index.parse().ok()?))
}

fn parse_device_segment(segment: &str) -> Option<(u16, u16)> {
    if segment.len() != 17 || !segment.is_ascii() {
        return None;
    }
    let (vid_label, rest) = segment.split_at(4);
    let (vid, rest) = rest.split_at(4);
    let (pid_label, pid) = rest.split_at(5);
    if !vid_label.eq_ignore_ascii_case("VID_") || !pid_label.eq_ignore_ascii_case("&PID_") {
        return None;
    }
    let hex4 = |text: &str| {
        if text.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            u16::from_str_radix(text, 16).ok()
        } else {
            None
        }
    };
    Some((hex4(vid)?, hex4(pid)?))
}

#[cfg(test)]
mod tests {
    use super::*;

    const STEAM_GUID: [u8; 16] = [
        0xd2, 0xf6, 0x48, 0xbb, 0x1b, 0xc3, 0xf1, 0x11, 0x80, 0x01, 0x44, 0x45, 0x53, 0x54, 0x00,
        0x00,
    ];

    fn header(device: &str, index: u32) -> String {
        format!(
            "[System\\\\CurrentControlSet\\\\Control\\\\MediaProperties\\\\PrivateProperties\\\\DirectInput\\\\{device}\\\\Calibration\\\\{index}]"
        )
    }

    fn guid_line(bytes: &[u8]) -> String {
        let hex: Vec<String> = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
        format!("\"GUID\"=hex:{}", hex.join(","))
    }

    #[test]
    fn preserved_wine_prefix_entry_is_parsed() {
        let text = r#"WINE REGISTRY Version 2
;; All keys relative to \\User\\S-1-5-21-0-0-0-1000

[System\\CurrentControlSet\\Control\\MediaProperties\\PrivateProperties\\DirectInput\\VID_045E&PID_028E\\Calibration\\0] 1791465929
#time=1dd57287d06dab6
"GUID"=hex:d2,f6,48,bb,1b,c3,f1,11,80,01,44,45,53,54,00,00
"Joystick Id"=hex:00,00,00,00
"#;
        assert_eq!(
            parse_directinput_cache(text),
            vec![CachedPad {
                vendor: 0x045E,
                product: 0x028E,
                index: 0,
                instance: STEAM_GUID,
            }]
        );
    }

    #[test]
    fn several_indexes_and_devices_keep_file_order() {
        let text = format!(
            "{}\n{}\n\n{}\n{}\n\n{}\n{}\n",
            header("VID_045E&PID_028E", 1),
            guid_line(&[1; 16]),
            header("VID_28DE&PID_11FF", 0),
            guid_line(&[2; 16]),
            header("VID_045E&PID_028E", 0),
            guid_line(&[3; 16]),
        );
        let pads = parse_directinput_cache(&text);
        let summary: Vec<_> = pads
            .iter()
            .map(|pad| (pad.vendor, pad.product, pad.index, pad.instance[0]))
            .collect();
        assert_eq!(
            summary,
            vec![
                (0x045E, 0x028E, 1, 1),
                (0x28DE, 0x11FF, 0, 2),
                (0x045E, 0x028E, 0, 3),
            ]
        );
    }

    #[test]
    fn sections_without_a_guid_are_skipped() {
        let text = format!(
            "{}\n\"Joystick Id\"=hex:00,00,00,00\n\n{}\n{}\n",
            header("VID_045E&PID_028E", 0),
            header("VID_045E&PID_028E", 1),
            guid_line(&[7; 16]),
        );
        let pads = parse_directinput_cache(&text);
        assert_eq!(pads.len(), 1);
        assert_eq!(pads[0].index, 1);
    }

    #[test]
    fn invalid_hex_is_skipped() {
        let valid = "01,02,03,04,05,06,07,08,09,0a,0b,0c,0d,0e,0f,10";
        for bad in [
            "01,02,03,04,05,06,07,08,09,0a,0b,0c,0d,0e,0f,1",
            "01,02,03,04,05,06,07,08,09,0a,0b,0c,0d,0e,0f,zz",
            "01,02,03,04,05,06,07,08,09,0a,0b,0c,0d,0e,0f,+1",
            "01,02,03,04,05,06,07,08,09,0a,0b,0c,0d,0e,0f,100",
            "01,02,03,04,05,06,07,08,09,0a,0b,0c,0d,0e,0f,10,",
            "01 02 03 04 05 06 07 08 09 0a 0b 0c 0d 0e 0f 10",
            "",
        ] {
            let text = format!(
                "{}\n\"GUID\"=hex:{bad}\n\n{}\n\"GUID\"=hex:{valid}\n",
                header("VID_045E&PID_028E", 0),
                header("VID_045E&PID_028E", 1),
            );
            let pads = parse_directinput_cache(&text);
            assert_eq!(pads.len(), 1, "accepted {bad:?}");
            assert_eq!(pads[0].index, 1);
        }
    }

    #[test]
    fn only_sixteen_byte_guids_are_accepted() {
        for length in [15usize, 17] {
            let text = format!(
                "{}\n{}\n",
                header("VID_045E&PID_028E", 0),
                guid_line(&vec![9; length])
            );
            assert!(parse_directinput_cache(&text).is_empty(), "{length} bytes");
        }
    }

    #[test]
    fn other_value_types_do_not_count_as_a_guid() {
        let text = format!(
            "{}\n\"GUID\"=\"d2f648bb\"\n\"Other\"=hex:{}\n",
            header("VID_045E&PID_028E", 0),
            "01,".repeat(15) + "01",
        );
        assert!(parse_directinput_cache(&text).is_empty());
    }

    #[test]
    fn names_and_hex_digits_are_case_insensitive() {
        let text = format!(
            "{}\n\"guid\"=hex:D2,F6,48,BB,1B,C3,F1,11,80,01,44,45,53,54,00,00\n",
            header("vid_045e&pid_028e", 0).replace("DirectInput", "directinput"),
        );
        let pads = parse_directinput_cache(&text);
        assert_eq!(pads.len(), 1);
        assert_eq!(
            (pads[0].vendor, pads[0].product, pads[0].instance),
            (0x045E, 0x028E, STEAM_GUID)
        );
    }

    #[test]
    fn headers_parse_with_and_without_a_timestamp() {
        let body = guid_line(&[4; 16]);
        let plain = format!("{}\n{body}\n", header("VID_045E&PID_028E", 0));
        let stamped = format!("{} 1791465929\n{body}\n", header("VID_045E&PID_028E", 0));
        let spaced = format!(
            "{}   1791465929  \n{body}\n",
            header("VID_045E&PID_028E", 0)
        );
        let bad_stamp = format!("{} soon\n{body}\n", header("VID_045E&PID_028E", 0));
        assert_eq!(parse_directinput_cache(&plain).len(), 1);
        assert_eq!(parse_directinput_cache(&stamped).len(), 1);
        assert_eq!(parse_directinput_cache(&spaced).len(), 1);
        assert!(parse_directinput_cache(&bad_stamp).is_empty());
    }

    #[test]
    fn a_guid_split_by_line_continuation_is_joined() {
        let text = format!(
            "{}\r\n\"GUID\"=hex:d2,f6,48,bb,1b,c3,f1,11,\\\r\n  80,01,44,45,\\\r\n  53,54,00,00\r\n\"Joystick Id\"=hex:00,00,00,00\r\n",
            header("VID_045E&PID_028E", 0),
        );
        let pads = parse_directinput_cache(&text);
        assert_eq!(pads.len(), 1);
        assert_eq!(pads[0].instance, STEAM_GUID);
    }

    #[test]
    fn comments_and_unrelated_sections_are_ignored() {
        let text = format!(
            "{}\n#time=1dd57287d06dab6\n{}\n\n[Software\\\\Wine\\\\DirectInput] 1\n\"GUID\"=hex:{}\n\n{}\n{}\n",
            header("VID_045E&PID_028E", 0),
            guid_line(&[1; 16]),
            "05,".repeat(15) + "05",
            header("VID_28DE&PID_11FF", 0),
            guid_line(&[2; 16]),
        );
        let pads = parse_directinput_cache(&text);
        assert_eq!(pads.len(), 2);
        assert_eq!(pads[0].instance, [1; 16]);
        assert_eq!(pads[1].instance, [2; 16]);
    }

    #[test]
    fn near_miss_keys_are_ignored() {
        let guid = guid_line(&[1; 16]);
        let wrong_keys = [
            header("VID_045E&PID_028", 0),
            header("VID_045E&PID_028EE", 0),
            header("VID_045G&PID_028E", 0),
            header("VID_045E&XID_028E", 0),
            header("VID_045E&PID_028E", 0).replace("Calibration", "Calibrations"),
            header("VID_045E&PID_028E", 0).replace("\\\\0]", "\\\\x]"),
            header("VID_045E&PID_028E", 0).replace("\\\\Calibration\\\\0]", "]"),
            header("VID_045E&PID_028E", 0).replace("\\\\0]", "\\\\0\\\\1]"),
            header("VID_045E&PID_028E", 0).replace("\\\\0]", "\\\\99999999999]"),
            header("VID_045E&PID_028E", 0).replace("PrivateProperties", "Properties"),
        ];
        for key in wrong_keys {
            assert!(
                parse_directinput_cache(&format!("{key}\n{guid}\n")).is_empty(),
                "{key}"
            );
        }
    }

    #[test]
    fn entries_are_capped() {
        let text: String = (0..80)
            .map(|index| {
                format!(
                    "{}\n{}\n",
                    header("VID_045E&PID_028E", index),
                    guid_line(&[1; 16])
                )
            })
            .collect();
        let pads = parse_directinput_cache(&text);
        assert_eq!(pads.len(), MAX_ENTRIES);
        assert_eq!(pads[MAX_ENTRIES - 1].index, 63);
    }
}
