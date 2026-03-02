use serde::Serialize;

/// Descriptor info sent to the frontend — both views are pre-formatted text.
#[derive(Debug, Clone, Serialize)]
pub struct HidDescriptorInfo {
    /// Plain hex bytes, 16 per line.
    pub raw_hex: String,
    /// Annotated descriptor: hex bytes with inline comments like source code.
    pub annotated: String,
}

/// Parse raw HID descriptor bytes into display-ready strings.
pub fn parse_hid_descriptor(raw: &[u8]) -> HidDescriptorInfo {
    HidDescriptorInfo {
        raw_hex: format_raw_hex(raw),
        annotated: annotate_descriptor(raw),
    }
}

// ---------------------------------------------------------------------------
// Raw hex: just the bytes, 16 per line
// ---------------------------------------------------------------------------

fn format_raw_hex(raw: &[u8]) -> String {
    if raw.is_empty() {
        return "(empty descriptor)".to_string();
    }
    raw.chunks(16)
        .map(|chunk| {
            chunk
                .iter()
                .map(|b| format!("{b:02X}"))
                .collect::<Vec<_>>()
                .join(" ")
        })
        .collect::<Vec<_>>()
        .join("\n")
}

// ---------------------------------------------------------------------------
// Annotated descriptor: parse HID items, format like source code
// ---------------------------------------------------------------------------

fn annotate_descriptor(raw: &[u8]) -> String {
    if raw.is_empty() {
        return "(empty descriptor)".to_string();
    }

    let mut lines = Vec::new();
    let mut offset = 0;
    let mut indent: usize = 0;
    let mut current_usage_page: u16 = 0;

    while offset < raw.len() {
        let prefix = raw[offset];

        // Long items (bSize=3 means 4 data bytes, but prefix 0xFE = long item).
        // Long items are rare; just dump them raw.
        if prefix == 0xFE {
            lines.push(format!("0x{prefix:02X}, ...          // (long item)"));
            break;
        }

        let bsize = match prefix & 0x03 {
            0 => 0usize,
            1 => 1,
            2 => 2,
            3 => 4,
            _ => unreachable!(),
        };
        let btype = (prefix >> 2) & 0x03;
        let btag = (prefix >> 4) & 0x0F;

        // Bounds check.
        if offset + 1 + bsize > raw.len() {
            let remaining: Vec<String> = raw[offset..]
                .iter()
                .map(|b| format!("0x{b:02X}"))
                .collect();
            lines.push(format!("{}  // (truncated)", remaining.join(", ")));
            break;
        }

        let data = &raw[offset + 1..offset + 1 + bsize];
        let uval = read_unsigned(data);
        let sval = read_signed(data, bsize);

        // Format hex bytes for this item.
        let hex_bytes: Vec<String> = raw[offset..offset + 1 + bsize]
            .iter()
            .map(|b| format!("0x{b:02X}"))
            .collect();
        let hex = format!("{},", hex_bytes.join(", "));

        // End Collection decreases indent before printing.
        if btype == 0 && btag == 0x0C {
            indent = indent.saturating_sub(1);
        }

        let comment_indent = "  ".repeat(indent);
        let comment = describe_item(btype, btag, uval, sval, current_usage_page);

        // Track global state.
        if btype == 1 && btag == 0x0 {
            current_usage_page = uval as u16;
        }

        lines.push(format!("{hex:<28}// {comment_indent}{comment}"));

        // Collection increases indent after printing.
        if btype == 0 && btag == 0x0A {
            indent += 1;
        }

        offset += 1 + bsize;
    }

    lines.join("\n")
}

fn describe_item(btype: u8, btag: u8, uval: u32, sval: i32, current_page: u16) -> String {
    match btype {
        0 => describe_main(btag, uval),
        1 => describe_global(btag, uval, sval),
        2 => describe_local(btag, uval, current_page),
        _ => format!("Reserved (0x{:02X})", (btag << 4) | (btype << 2)),
    }
}

// ---------------------------------------------------------------------------
// Main items
// ---------------------------------------------------------------------------

fn describe_main(btag: u8, val: u32) -> String {
    match btag {
        0x08 => format!("Input ({})", bitfield_flags(val)),
        0x09 => format!("Output ({})", bitfield_flags(val)),
        0x0B => format!("Feature ({})", bitfield_flags(val)),
        0x0A => format!("Collection ({})", collection_type(val)),
        0x0C => "End Collection".to_string(),
        _ => format!("Main(0x{btag:X}, 0x{val:X})"),
    }
}

fn bitfield_flags(val: u32) -> String {
    let mut flags = Vec::new();
    flags.push(if val & 0x01 != 0 { "Cnst" } else { "Data" });
    flags.push(if val & 0x02 != 0 { "Var" } else { "Ary" });
    flags.push(if val & 0x04 != 0 { "Rel" } else { "Abs" });
    if val & 0x08 != 0 {
        flags.push("Wrap");
    }
    if val & 0x10 != 0 {
        flags.push("NLin");
    }
    if val & 0x20 != 0 {
        flags.push("NoPref");
    }
    if val & 0x40 != 0 {
        flags.push("Null");
    }
    if val & 0x80 != 0 {
        flags.push("Vol");
    }
    if val & 0x100 != 0 {
        flags.push("Buf");
    }
    flags.join(",")
}

fn collection_type(val: u32) -> &'static str {
    match val {
        0x00 => "Physical",
        0x01 => "Application",
        0x02 => "Logical",
        0x03 => "Report",
        0x04 => "Named Array",
        0x05 => "Usage Switch",
        0x06 => "Usage Modifier",
        _ => "Vendor",
    }
}

// ---------------------------------------------------------------------------
// Global items
// ---------------------------------------------------------------------------

fn describe_global(btag: u8, uval: u32, sval: i32) -> String {
    match btag {
        0x0 => format!("Usage Page ({})", usage_page_name(uval as u16)),
        0x1 => format!("Logical Minimum ({sval})"),
        0x2 => format!("Logical Maximum ({sval})"),
        0x3 => format!("Physical Minimum ({sval})"),
        0x4 => format!("Physical Maximum ({sval})"),
        0x5 => format!("Unit Exponent ({sval})"),
        0x6 => format!("Unit (0x{uval:X})"),
        0x7 => format!("Report Size ({uval})"),
        0x8 => format!("Report ID ({uval})"),
        0x9 => format!("Report Count ({uval})"),
        0xA => "Push".to_string(),
        0xB => "Pop".to_string(),
        _ => format!("Global(0x{btag:X}, 0x{uval:X})"),
    }
}

// ---------------------------------------------------------------------------
// Local items
// ---------------------------------------------------------------------------

fn describe_local(btag: u8, uval: u32, current_page: u16) -> String {
    match btag {
        0x0 => {
            // Usage — could be 16-bit (just ID) or 32-bit (page + ID).
            if uval > 0xFFFF {
                let page = (uval >> 16) as u16;
                let id = uval as u16;
                format!("Usage ({} > {})", usage_page_name(page), usage_name(page, id))
            } else {
                format!(
                    "Usage ({})",
                    usage_name(current_page, uval as u16)
                )
            }
        }
        0x1 => format!("Usage Minimum (0x{uval:X})"),
        0x2 => format!("Usage Maximum (0x{uval:X})"),
        0x3 => format!("Designator Index ({uval})"),
        0x4 => format!("Designator Minimum ({uval})"),
        0x5 => format!("Designator Maximum ({uval})"),
        0x7 => format!("String Index ({uval})"),
        0x8 => format!("String Minimum ({uval})"),
        0x9 => format!("String Maximum ({uval})"),
        0xA => format!("Delimiter ({})", if uval == 1 { "Open" } else { "Close" }),
        _ => format!("Local(0x{btag:X}, 0x{uval:X})"),
    }
}

// ---------------------------------------------------------------------------
// Usage name resolution via hut
// ---------------------------------------------------------------------------

fn usage_page_name(page: u16) -> String {
    hut::UsagePage::try_from(page)
        .map(|p| camel_to_spaced(&format!("{p:?}")))
        .unwrap_or_else(|_| {
            if page >= 0xFF00 {
                format!("Vendor 0x{page:04X}")
            } else {
                format!("0x{page:04X}")
            }
        })
}

fn usage_name(page: u16, id: u16) -> String {
    let combined = ((page as u32) << 16) | id as u32;
    hut::Usage::try_from(combined)
        .map(|u| camel_to_spaced(&format!("{u:?}")))
        .unwrap_or_else(|_| format!("0x{id:04X}"))
}

/// Convert CamelCase to "Camel Case". Handles runs of capitals like "VR" or "LED".
fn camel_to_spaced(s: &str) -> String {
    // Strip outer enum wrapper like "GenericDesktop(Mouse)" → just take the inner part
    // or "GenericDesktop" → use as-is.
    let s = if let Some(idx) = s.find('(') {
        // e.g. "GenericDesktop(Gamepad)" → "Gamepad"
        &s[idx + 1..s.len() - 1]
    } else {
        s
    };

    let chars: Vec<char> = s.chars().collect();
    let mut result = String::new();
    for (i, &c) in chars.iter().enumerate() {
        if i > 0 && c.is_uppercase() {
            let prev = chars[i - 1];
            let next_is_lower = chars.get(i + 1).is_some_and(|n| n.is_lowercase());
            if prev.is_lowercase() || (prev.is_uppercase() && next_is_lower) {
                result.push(' ');
            }
        }
        result.push(c);
    }
    result
}

// ---------------------------------------------------------------------------
// Byte reading helpers
// ---------------------------------------------------------------------------

fn read_unsigned(data: &[u8]) -> u32 {
    match data.len() {
        0 => 0,
        1 => data[0] as u32,
        2 => u16::from_le_bytes([data[0], data[1]]) as u32,
        4 => u32::from_le_bytes([data[0], data[1], data[2], data[3]]),
        _ => 0,
    }
}

fn read_signed(data: &[u8], size: usize) -> i32 {
    match size {
        0 => 0,
        1 => data[0] as i8 as i32,
        2 => i16::from_le_bytes([data[0], data[1]]) as i32,
        4 => i32::from_le_bytes([data[0], data[1], data[2], data[3]]),
        _ => 0,
    }
}
