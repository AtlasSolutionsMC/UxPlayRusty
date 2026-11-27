//! Terminal rendering for the four-digit access PIN shown by UxPlay.

const DIGITS: [[&str; 8]; 10] = [
    [
        "0821111380",
        "2114005113",
        "1110000111",
        "1110000111",
        "1110000111",
        "1110000111",
        "5113002114",
        "0751111470",
    ],
    [
        "0002111000",
        "0021111000",
        "0000111000",
        "0000111000",
        "0000111000",
        "0000111000",
        "0000111000",
        "0011111110",
    ],
    [
        "0811112800",
        "2114005113",
        "0000000111",
        "0000082114",
        "0862111470",
        "2114700000",
        "1117000000",
        "1111111111",
    ],
    [
        "0821111380",
        "2114005113",
        "0000082114",
        "0000111170",
        "0000075130",
        "1110000111",
        "5113002114",
        "0751111470",
    ],
    [
        "0000211110",
        "0001401110",
        "0021401110",
        "0214001110",
        "2110001110",
        "1111111111",
        "0000001110",
        "0000001110",
    ],
    [
        "1111111110",
        "1110000000",
        "1110000000",
        "1112111380",
        "0000075113",
        "0000000111",
        "5113002114",
        "0711114700",
    ],
    [
        "0821111380",
        "2114005113",
        "1110000000",
        "1112111380",
        "1114075113",
        "1110000111",
        "5113002114",
        "0751111470",
    ],
    [
        "1111111111",
        "0000002114",
        "0000021140",
        "0000211400",
        "0002114000",
        "0021140000",
        "0211400000",
        "2114000000",
    ],
    [
        "0831111280",
        "2114002114",
        "5113802114",
        "0751111170",
        "8214775138",
        "1110000111",
        "5113002114",
        "0751111470",
    ],
    [
        "0821111380",
        "2114005113",
        "1110000111",
        "5113802111",
        "0751114111",
        "0000000111",
        "5113002114",
        "0751111470",
    ],
];

const PIXELS: [char; 9] = [' ', '8', 'd', 'b', 'P', 'Y', 'o', '"', '.'];

pub fn render_pin(pin: &str) -> Option<String> {
    if pin.is_empty() || !pin.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let digits = pin
        .bytes()
        .map(|byte| usize::from(byte - b'0'))
        .collect::<Vec<_>>();
    let mut rows = vec![String::new(); 8];
    for (index, digit) in digits.iter().enumerate() {
        for (row, glyph) in rows.iter_mut().zip(&DIGITS[*digit]) {
            if index != 0 {
                row.push_str("   ");
            }
            row.extend(glyph.bytes().map(|pixel| PIXELS[usize::from(pixel - b'0')]));
        }
    }
    let mut output = String::from("\n");
    for (index, row) in rows.iter().enumerate() {
        output.push_str("          ");
        output.push_str(row);
        if index + 1 != rows.len() {
            output.push('\n');
        }
    }
    Some(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_four_digit_pins_with_the_source_font() {
        let image = render_pin("0042").unwrap();
        assert_eq!(image.lines().skip(1).count(), 8);
        assert!(image.contains("          "));
        assert!(render_pin("12x4").is_none());
    }
}
