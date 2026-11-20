/// Splits an Annex B byte stream into NAL units without their start codes.
pub fn nal_units(data: &[u8]) -> Vec<&[u8]> {
    let mut starts = Vec::new();
    let mut i = 0;
    while i + 3 <= data.len() {
        if data[i] == 0 && data[i + 1] == 0 && data[i + 2] == 1 {
            starts.push(i + 3);
            i += 3;
        } else {
            i += 1;
        }
    }

    let mut units = Vec::with_capacity(starts.len());
    for (n, &start) in starts.iter().enumerate() {
        let mut end = if n + 1 < starts.len() {
            starts[n + 1] - 3
        } else {
            data.len()
        };
        while end > start && data[end - 1] == 0 {
            end -= 1;
        }
        if end > start {
            units.push(&data[start..end]);
        }
    }
    units
}

/// The WebCodecs codec string, e.g. "avc1.42e028", from the SPS in an access unit.
pub fn codec_string(data: &[u8]) -> Option<String> {
    nal_units(data)
        .into_iter()
        .find(|unit| unit[0] & 0x1f == 7 && unit.len() >= 4)
        .map(|sps| format!("avc1.{:02x}{:02x}{:02x}", sps[1], sps[2], sps[3]))
}

pub fn is_key_frame(data: &[u8]) -> bool {
    nal_units(data).iter().any(|unit| unit[0] & 0x1f == 5)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_codec_and_key_frame() {
        let data = [
            0, 0, 0, 1, 0x67, 0x42, 0xe0, 0x28, 0xaa, 0, 0, 0, 1, 0x68, 0xce, 0, 0, 1, 0x65, 0x88,
        ];
        assert_eq!(codec_string(&data).as_deref(), Some("avc1.42e028"));
        assert!(is_key_frame(&data));
        assert_eq!(nal_units(&data).len(), 3);
    }
}
