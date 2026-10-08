//! BITS's strings: a 32-bit count of UTF-16 code units, the terminating
//! NUL included, then the units. An empty string is a count of one and a
//! NUL.

use std::ops::Range;

use common::bytes::Reader;
use common::text::utf16le;

/// The longest string read, in code units: Windows paths reach 32,767
/// characters, and URLs that long are read too.
const MAX_UNITS: usize = 65_536;
/// Bytes of the count.
pub(crate) const COUNT_SIZE: usize = 4;
/// Bytes of a code unit.
const UNIT_SIZE: usize = 2;

/// The string `reader` is at, moving past it; `None` (the reader then
/// somewhere past where it was) when the bytes there aren't one: a count of
/// zero or beyond [`MAX_UNITS`], units past the end, a NUL before the last
/// unit or none at the end.
pub(crate) fn read(reader: &mut Reader<'_>) -> Option<String> {
    let units = usize::try_from(reader.u32_le().ok()?).ok()?;
    if !(1..=MAX_UNITS).contains(&units) {
        return None;
    }
    decode(reader.bytes(units * UNIT_SIZE).ok()?)
}

/// The string that ends where `within` does and starts inside it, and
/// where it starts in `data`; `None` when no count frames one. The shortest
/// string that fits is taken.
pub(crate) fn ending_at(data: &[u8], within: Range<usize>) -> Option<(usize, String)> {
    let region = data.get(within.clone())?;
    let most = (region.len().saturating_sub(COUNT_SIZE) / UNIT_SIZE).min(MAX_UNITS);
    (1..=most).find_map(|units| {
        let start = region.len() - COUNT_SIZE - units * UNIT_SIZE;
        let count = u32::from_le_bytes(region[start..start + COUNT_SIZE].try_into().ok()?);
        if usize::try_from(count).ok()? != units {
            return None;
        }
        decode(&region[start + COUNT_SIZE..]).map(|text| (within.start + start, text))
    })
}

/// The longest tail of `data[within]` that reads as text a path or URL
/// holds (printable Latin characters), ending at a NUL unit; for a string
/// whose start was overwritten. `None` when not even one character does.
pub(crate) fn tail_ending_at(data: &[u8], within: Range<usize>) -> Option<String> {
    let region = data.get(within)?;
    let units = region.strip_suffix(&[0, 0])?;
    let mut start = units.len();
    while start >= UNIT_SIZE {
        let unit = u16::from_le_bytes([units[start - 2], units[start - 1]]);
        if !is_path_character(unit) {
            break;
        }
        start -= UNIT_SIZE;
    }
    (start < units.len()).then(|| utf16le(&units[start..]).text)
}

/// Printable Basic Latin, Latin-1 and Latin Extended-A and B characters.
fn is_path_character(unit: u16) -> bool {
    const FIRST_PRINTABLE: u16 = 0x20;
    const DELETE: u16 = 0x7F;
    const FIRST_LATIN_1: u16 = 0xA0;
    const LAST_LATIN_EXTENDED_B: u16 = 0x24F;
    (FIRST_PRINTABLE..DELETE).contains(&unit)
        || (FIRST_LATIN_1..=LAST_LATIN_EXTENDED_B).contains(&unit)
}

/// The units of a string, NUL last: the text before it.
fn decode(units: &[u8]) -> Option<String> {
    let (text, nul) = units.split_at(units.len().checked_sub(UNIT_SIZE)?);
    let has_inner_nul = text.chunks_exact(UNIT_SIZE).any(|unit| unit == [0, 0]);
    (nul == [0, 0] && !has_inner_nul).then(|| utf16le(text).text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::record::tests::stored;

    #[test]
    fn reads_counted_strings() {
        let mut data = stored("C:\\x.tmp");
        data.extend(stored(""));
        let mut reader = Reader::new(&data);
        assert_eq!(read(&mut reader).as_deref(), Some("C:\\x.tmp"));
        assert_eq!(read(&mut reader).as_deref(), Some(""));
        assert_eq!(reader.remaining(), 0);
    }

    #[test]
    fn rejects_what_isnt_one() {
        for bad in [
            vec![0, 0, 0, 0],
            vec![2, 0, 0, 0, b'a', 0, b'b', 0],
            vec![2, 0, 0, 0, 0, 0, 0, 0],
            vec![3, 0, 0, 0, b'a', 0],
            vec![0xFF, 0xFF, 0xFF, 0xFF],
        ] {
            assert_eq!(read(&mut Reader::new(&bad)), None, "{bad:?}");
        }
    }

    #[test]
    fn reads_backwards() {
        let mut data = vec![0xAA; 3];
        data.extend(stored("S-1-5-18"));
        data.extend(stored(""));
        let end = data.len();
        let (start, empty) = ending_at(&data, 0..end).unwrap();
        assert_eq!(empty, "");
        assert_eq!(ending_at(&data, 0..start), Some((3, "S-1-5-18".to_owned())));
        assert_eq!(
            ending_at(&data, 4..start),
            None,
            "it starts before the range"
        );
        assert_eq!(ending_at(&data, 0..3), None);
        assert_eq!(ending_at(&data, 0..end + 1), None);
    }

    #[test]
    fn keeps_the_end_of_an_overwritten_string() {
        let mut data = vec![0x9B, 0x76];
        data.extend("cf15\\BIT1.tmp\0".encode_utf16().flat_map(u16::to_le_bytes));
        assert_eq!(
            tail_ending_at(&data, 0..data.len()).as_deref(),
            Some("cf15\\BIT1.tmp")
        );
        assert_eq!(tail_ending_at(&[0x9B, 0x76, 0, 0], 0..4), None);
        assert_eq!(tail_ending_at(b"ab", 0..2), None);
    }
}
