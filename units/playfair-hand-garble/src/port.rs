//! Ports the scalar helpers and the fixed-size buffer transform from `hand_garble.c`.

#![forbid(unsafe_code)]
#[derive(Clone, Copy)]
struct CUnsigned(u32);

impl From<u32> for CUnsigned {
    fn from(value: u32) -> Self {
        Self(value)
    }
}

macro_rules! impl_wrapping_binary_op {
    ($trait:ident, $method:ident, $wrapped:ident) => {
        impl std::ops::$trait for CUnsigned {
            type Output = Self;

            fn $method(self, rhs: Self) -> Self::Output {
                Self(self.0.$wrapped(rhs.0))
            }
        }

        impl std::ops::$trait<u32> for CUnsigned {
            type Output = Self;

            fn $method(self, rhs: u32) -> Self::Output {
                Self(self.0.$wrapped(rhs))
            }
        }
    };
}

impl_wrapping_binary_op!(Add, add, wrapping_add);
impl_wrapping_binary_op!(Sub, sub, wrapping_sub);
impl_wrapping_binary_op!(Mul, mul, wrapping_mul);

macro_rules! impl_unsigned_binary_op {
    ($trait:ident, $method:ident, $op:tt) => {
        impl std::ops::$trait for CUnsigned {
            type Output = Self;

            fn $method(self, rhs: Self) -> Self::Output {
                Self(self.0 $op rhs.0)
            }
        }

        impl std::ops::$trait<u32> for CUnsigned {
            type Output = Self;

            fn $method(self, rhs: u32) -> Self::Output {
                Self(self.0 $op rhs)
            }
        }
    };
}

impl_unsigned_binary_op!(Div, div, /);
impl_unsigned_binary_op!(Rem, rem, %);
impl_unsigned_binary_op!(BitAnd, bitand, &);
impl_unsigned_binary_op!(BitOr, bitor, |);
impl_unsigned_binary_op!(BitXor, bitxor, ^);

impl std::ops::Not for CUnsigned {
    type Output = Self;

    fn not(self) -> Self::Output {
        Self(!self.0)
    }
}

impl std::ops::Shl for CUnsigned {
    type Output = Self;

    fn shl(self, rhs: Self) -> Self::Output {
        Self(self.0.wrapping_shl(rhs.0))
    }
}

impl std::ops::Shl<u32> for CUnsigned {
    type Output = Self;

    fn shl(self, rhs: u32) -> Self::Output {
        Self(self.0.wrapping_shl(rhs))
    }
}

impl std::ops::Shr for CUnsigned {
    type Output = Self;

    fn shr(self, rhs: Self) -> Self::Output {
        Self(self.0.wrapping_shr(rhs.0))
    }
}

impl std::ops::Shr<u32> for CUnsigned {
    type Output = Self;

    fn shr(self, rhs: u32) -> Self::Output {
        Self(self.0.wrapping_shr(rhs))
    }
}

/// Port of `uint32_t weird_ror8(uint8_t input, int count)` (`hand_garble.c:9`).
pub fn weird_ror8(input: u8, count: std::ffi::c_int) -> u32 {
    if count == 0 {
        return 0;
    }
    if !(1..=8).contains(&count) {
        return 0;
    }

    (u32::from(input) >> count as u32) | (u32::from(input) << (8 - count as u32))
}

/// Port of `uint32_t weird_rol8(uint8_t input, int count)` (`hand_garble.c:17`).
pub fn weird_rol8(input: u8, count: std::ffi::c_int) -> u32 {
    if count == 0 {
        return 0;
    }
    if !(1..=8).contains(&count) {
        return 0;
    }

    ((u32::from(input) << count as u32) & 0xff) | (u32::from(input) >> (8 - count as u32))
}

/// Port of `uint32_t weird_rol32(uint8_t input, int count)` (`hand_garble.c:24`).
pub fn weird_rol32(input: u8, count: std::ffi::c_int) -> u32 {
    if count == 0 {
        return 0;
    }
    if !(1..=8).contains(&count) {
        return 0;
    }

    (u32::from(input) << count as u32) ^ (u32::from(input) >> (8 - count as u32))
}

/// Safe fixed-size port of `garble` (`hand_garble.c:33`). The source caller supplies buffers of
/// exactly these lengths. `buffer4` is read-only in the C implementation.
///
/// The C routine performs its arithmetic in `unsigned int`. `CUnsigned` makes modulo-2^32 behavior
/// explicit, regardless of the overflow-check profile of a downstream crate.
pub fn garble(
    buffer0: &mut [u8; 20],
    buffer1: &mut [u8; 210],
    buffer2: &mut [u8; 35],
    buffer3: &mut [u8; 132],
    buffer4: &[u8; 21],
) {
    macro_rules! v {
        ($array:ident, $index:expr) => {
            CUnsigned(u32::from($array[$index]))
        };
    }
    macro_rules! put {
        ($array:ident, $index:expr, $value:expr $(,)?) => {{
            let value: CUnsigned = ($value).into();
            $array[$index] = value.0 as u8;
        }};
    }
    macro_rules! add {
        ($array:ident, $index:expr, $value:expr $(,)?) => {{
            let value: CUnsigned = ($value).into();
            $array[$index] = u32::from($array[$index]).wrapping_add(value.0) as u8;
        }};
    }
    macro_rules! sub {
        ($array:ident, $index:expr, $value:expr $(,)?) => {{
            let value: CUnsigned = ($value).into();
            $array[$index] = u32::from($array[$index]).wrapping_sub(value.0) as u8;
        }};
    }
    macro_rules! xor {
        ($array:ident, $index:expr, $value:expr $(,)?) => {{
            let value: CUnsigned = ($value).into();
            $array[$index] = (u32::from($array[$index]) ^ value.0) as u8;
        }};
    }
    fn index(value: CUnsigned, length: u32) -> usize {
        (value.0 % length) as usize
    }
    fn rol8(input: CUnsigned, count: u32) -> CUnsigned {
        CUnsigned(u32::from((input.0 as u8).rotate_left(count)))
    }
    fn rol8x(input: CUnsigned, count: u32) -> CUnsigned {
        let input = u32::from(input.0 as u8);
        CUnsigned(input.wrapping_shl(count) | input.wrapping_shr(8 - count))
    }

    let (mut a, mut b, mut c, mut d, mut e): (
        CUnsigned,
        CUnsigned,
        CUnsigned,
        CUnsigned,
        CUnsigned,
    );
    let (m, j, mut g, mut f): (CUnsigned, CUnsigned, CUnsigned, CUnsigned);
    let (h, k, r, s, t): (CUnsigned, CUnsigned, CUnsigned, CUnsigned, CUnsigned);
    let (u, v, w, x, y, z): (
        CUnsigned,
        CUnsigned,
        CUnsigned,
        CUnsigned,
        CUnsigned,
        CUnsigned,
    );

    put!(
        buffer2,
        12,
        CUnsigned(0x14)
            + (((v!(buffer1, 64) & 92) | ((v!(buffer1, 99) / 3) & 35))
                & v!(
                    buffer4,
                    index(rol8x(v!(buffer4, index(v!(buffer1, 206), 21)), 4) % 21, 21)
                ))
    );
    put!(
        buffer1,
        4,
        (v!(buffer1, 99) / 5) * (v!(buffer1, 99) / 5) * 2
    );
    put!(buffer2, 34, 0xb8);
    xor!(
        buffer1,
        153,
        v!(buffer2, index(v!(buffer1, 203), 35))
            * v!(buffer2, index(v!(buffer1, 203), 35))
            * v!(buffer1, 190)
    );
    sub!(
        buffer0,
        3,
        ((v!(buffer4, index(v!(buffer1, 205), 21)) >> 1) & 80) | 0xe6440
    );
    put!(buffer0, 16, 0x93);
    put!(buffer0, 13, 0x62);
    sub!(buffer1, 33, v!(buffer4, index(v!(buffer1, 36), 21)) & 0xf6);
    let tmp2 = v!(buffer2, index(v!(buffer1, 67), 35));
    put!(buffer2, 12, 0x07);
    let tmp = v!(buffer0, index(v!(buffer1, 181), 20));
    sub!(buffer1, 2, 3136);
    put!(buffer0, 19, v!(buffer4, index(v!(buffer1, 58), 21)));
    put!(
        buffer3,
        0,
        CUnsigned(92) - v!(buffer2, index(v!(buffer1, 32), 35))
    );
    put!(buffer3, 4, v!(buffer2, index(v!(buffer1, 15), 35)) + 0x9e);
    add!(
        buffer1,
        34,
        v!(
            buffer4,
            index((v!(buffer2, index(v!(buffer1, 15), 35)) + 0x9e) & 0xff, 21)
        ) / 5
    );
    add!(
        buffer0,
        19,
        CUnsigned(0xffff_fee6) - ((v!(buffer0, index(v!(buffer3, 4), 20)) >> 1) & 102)
    );
    let shift_byte = v!(buffer4, index(v!(buffer1, 190), 21));
    let left_count = (CUnsigned(7) - (shift_byte - 1)) & 7;
    let rotated_xor = (v!(buffer1, 72) >> (shift_byte & 7)) ^ (v!(buffer1, 72) << left_count);
    put!(
        buffer1,
        15,
        (CUnsigned(3) * (rotated_xor - (CUnsigned(3) * v!(buffer4, index(v!(buffer1, 126), 21)))))
            ^ v!(buffer1, 15)
    );
    xor!(
        buffer0,
        15,
        v!(buffer2, index(v!(buffer1, 181), 35))
            * v!(buffer2, index(v!(buffer1, 181), 35))
            * v!(buffer2, index(v!(buffer1, 181), 35))
    );
    xor!(buffer2, 4, v!(buffer1, 202) / 3);
    a = CUnsigned(92) - v!(buffer0, index(v!(buffer3, 0), 20));
    e = (a & 0xc6) | ((!v!(buffer1, 105)) & 0xc6) | (a & (!v!(buffer1, 105)));
    add!(buffer2, 1, e * e * e);
    xor!(
        buffer0,
        19,
        ((CUnsigned(224) | (v!(buffer4, index(v!(buffer1, 92), 21)) & 27))
            * v!(buffer2, index(v!(buffer1, 41), 35)))
            / 3
    );
    add!(buffer1, 140, weird_ror8(92, (v!(buffer1, 5) & 7).0 as i32));
    add!(
        buffer2,
        12,
        ((((!v!(buffer1, 4)) ^ v!(buffer2, index(v!(buffer1, 12), 35))) | v!(buffer1, 182)) & 192)
            | (((!v!(buffer1, 4)) ^ v!(buffer2, index(v!(buffer1, 12), 35))) & v!(buffer1, 182))
    );
    add!(buffer1, 36, 125);
    put!(
        buffer1,
        124,
        rol8x(
            (((CUnsigned(74) & v!(buffer1, 138))
                | ((CUnsigned(74) | v!(buffer1, 138)) & v!(buffer0, 15)))
                & v!(buffer0, index(v!(buffer1, 43), 20)))
                | (((CUnsigned(74) & v!(buffer1, 138))
                    | ((CUnsigned(74) | v!(buffer1, 138)) & v!(buffer0, 15))
                    | v!(buffer0, index(v!(buffer1, 43), 20)))
                    & 95),
            4
        )
    );
    put!(
        buffer3,
        8,
        ((v!(buffer0, index(v!(buffer3, 4), 20)) & 95)
            & ((v!(buffer4, index(v!(buffer1, 68), 21)) & 46) << 1)
            | 16)
            ^ 92,
    );
    a = v!(buffer1, 177) + v!(buffer4, index(v!(buffer1, 79), 21));
    d = (((a >> 1) | ((CUnsigned(3) * v!(buffer1, 148)) / 5)) & v!(buffer2, 1))
        | ((a >> 1) & ((CUnsigned(3) * v!(buffer1, 148)) / 5));
    put!(buffer3, 12, CUnsigned(0xffff_ffde) - d);
    a = CUnsigned(8) - (v!(buffer2, 22) & 7);
    b = v!(buffer1, 33) >> (a & 7);
    c = v!(buffer1, 33) << (v!(buffer2, 22) & 7);
    add!(
        buffer2,
        16,
        ((v!(buffer2, index(v!(buffer3, 0), 35)) & 159)
            | v!(buffer0, index(v!(buffer3, 4), 20))
            | 8)
            - ((b ^ c) | 128)
    );
    xor!(buffer0, 14, v!(buffer2, index(v!(buffer3, 12), 35)));
    a = CUnsigned(weird_rol8(
        v!(buffer4, index(v!(buffer0, index(v!(buffer1, 201), 20)), 21)).0 as u8,
        ((v!(buffer2, index(v!(buffer1, 112), 35)) << 1) & 7).0 as i32,
    ));
    d = (v!(buffer0, index(v!(buffer1, 208), 20)) & 131)
        | (v!(buffer0, index(v!(buffer1, 164), 20)) & 124);
    add!(buffer1, 19, (a & (d / 5)) | ((a | (d / 5)) & 37));
    put!(
        buffer2,
        8,
        weird_ror8(
            140,
            (((v!(buffer4, index(v!(buffer1, 45), 21)) + 92)
                * (v!(buffer4, index(v!(buffer1, 45), 21)) + 92))
                & 7)
            .0 as i32
        )
    );
    put!(buffer1, 190, 56);
    xor!(buffer2, 8, v!(buffer3, 0));
    put!(
        buffer1,
        53,
        !((v!(buffer0, index(v!(buffer1, 83), 20)) | 204) / 5)
    );
    add!(buffer0, 13, v!(buffer0, index(v!(buffer1, 41), 20)));
    put!(
        buffer0,
        10,
        ((v!(buffer2, index(v!(buffer3, 0), 35)) & v!(buffer1, 2))
            | ((v!(buffer2, index(v!(buffer3, 0), 35)) | v!(buffer1, 2)) & v!(buffer3, 12)))
            / 15
    );
    a = (((CUnsigned(56) | (v!(buffer4, index(v!(buffer1, 2), 21)) & 68))
        | v!(buffer2, index(v!(buffer3, 8), 35)))
        & 42)
        | (((v!(buffer4, index(v!(buffer1, 2), 21)) & 68) | CUnsigned(56))
            & v!(buffer2, index(v!(buffer3, 8), 35)));
    put!(buffer3, 16, (a * a) + 110);
    put!(buffer3, 20, CUnsigned(202) - v!(buffer3, 16));
    put!(buffer3, 24, v!(buffer1, 151));
    xor!(buffer2, 13, v!(buffer4, index(v!(buffer3, 0), 21)));
    b = ((v!(buffer2, index(v!(buffer1, 179), 35)) - 38) & 177) | (v!(buffer3, 12) & 177);
    c = (v!(buffer2, index(v!(buffer1, 179), 35)) - 38) & v!(buffer3, 12);
    put!(buffer3, 28, CUnsigned(30) + ((b | c) * (b | c)));
    put!(buffer3, 32, v!(buffer3, 28) + 62);
    a = ((v!(buffer3, 20) + (v!(buffer3, 0) & 74)) | !v!(buffer4, index(v!(buffer3, 0), 21))) & 121;
    b = (v!(buffer3, 20) + (v!(buffer3, 0) & 74)) & !v!(buffer4, index(v!(buffer3, 0), 21));
    let tmp3 = a | b;
    c = ((((a | b) ^ 0xffff_ffa6) | v!(buffer3, 0)) & 4)
        | (((a | b) ^ 0xffff_ffa6) & v!(buffer3, 0));
    put!(
        buffer1,
        47,
        (v!(buffer2, index(v!(buffer1, 89), 35)) + c) ^ v!(buffer1, 47)
    );
    put!(
        buffer3,
        36,
        ((rol8((tmp & 179) + 68, 2) & v!(buffer0, 3)) | (tmp2 & !v!(buffer0, 3))) - 15
    );
    xor!(buffer1, 123, 221);
    a = (v!(buffer4, index(v!(buffer3, 0), 21)) / 3) - v!(buffer2, index(v!(buffer3, 4), 35));
    c = (((v!(buffer3, 0) & 163) + 92) & 246) | (v!(buffer3, 0) & 92);
    e = ((c | v!(buffer3, 24)) & 54) | (c & v!(buffer3, 24));
    put!(buffer3, 40, a - e);
    put!(
        buffer3,
        44,
        tmp3 ^ 81 ^ (((v!(buffer3, 0) >> 1) & 101) + 26)
    );
    put!(buffer3, 48, v!(buffer2, index(v!(buffer3, 4), 35)) & 27);
    put!(buffer3, 52, 27);
    put!(buffer3, 56, 199);
    let left = ((v!(buffer3, 40) | v!(buffer3, 24)) & 177) | (v!(buffer3, 40) & v!(buffer3, 24));
    let right =
        ((v!(buffer3, 40) & v!(buffer3, 24)) | ((v!(buffer3, 40) | v!(buffer3, 24)) & 177)) & 199;
    let table20 = (v!(buffer4, index(v!(buffer3, 0), 20)) & 177) | 176;
    let table21 = v!(buffer4, index(v!(buffer3, 0), 21));
    let table_mask = table21 & !3;
    let left_part = left & (table20 | table_mask);
    let right_part = (((table21 & 1) + 176) | table_mask) & v!(buffer3, 56);
    let combined = ((left_part | right | right_part) & !v!(buffer3, 52)) | v!(buffer3, 48);
    put!(buffer3, 64, v!(buffer3, 4) + combined);
    xor!(buffer2, 33, v!(buffer1, 26));
    xor!(buffer1, 106, v!(buffer3, 20) ^ 133);
    put!(
        buffer2,
        30,
        ((v!(buffer3, 64) / 3) - (CUnsigned(275) | (v!(buffer3, 0) & 247)))
            ^ v!(buffer0, index(v!(buffer1, 122), 20))
    );
    put!(
        buffer1,
        22,
        (v!(buffer2, index(v!(buffer1, 90), 35)) & 95) | 68
    );
    a = (v!(buffer4, index(v!(buffer3, 36), 21)) & 184)
        | (v!(buffer2, index(v!(buffer3, 44), 35)) & !184);
    add!(buffer2, 18, (a * a * a) >> 1);
    sub!(buffer2, 5, v!(buffer4, index(v!(buffer1, 92), 21)));
    a = (((v!(buffer1, 41) & !24) | (v!(buffer2, index(v!(buffer1, 183), 35)) & 24))
        & (v!(buffer3, 16) + 53))
        | (v!(buffer3, 20) & v!(buffer2, index(v!(buffer3, 20), 35)));
    b = (v!(buffer1, 17) & (!v!(buffer3, 44)))
        | (v!(buffer0, index(v!(buffer1, 59), 20)) & v!(buffer3, 44));
    xor!(buffer2, 18, a * b);
    a = CUnsigned(weird_ror8(
        v!(buffer1, 11).0 as u8,
        (v!(buffer2, index(v!(buffer1, 28), 35)) & 7).0 as i32,
    )) & 7;
    b = (((v!(buffer0, index(v!(buffer1, 93), 20)) & !v!(buffer0, 14)) | (v!(buffer0, 14) & 150))
        & !28)
        | (v!(buffer1, 7) & 28);
    put!(
        buffer2,
        22,
        ((((b | CUnsigned(weird_rol8(
            v!(buffer2, index(v!(buffer3, 0), 35)).0 as u8,
            a.0 as i32
        ))) & v!(buffer2, 33))
            | (b & CUnsigned(weird_rol8(
                v!(buffer2, index(v!(buffer3, 0), 35)).0 as u8,
                a.0 as i32
            ))))
            + 74)
            & 0xff
    );
    a = v!(
        buffer4,
        index(v!(buffer0, index(v!(buffer1, 39), 20)) ^ 217, 21)
    );
    sub!(
        buffer0,
        15,
        ((((v!(buffer3, 20) | v!(buffer3, 0)) & 214) | (v!(buffer3, 20) & v!(buffer3, 0))) & a)
            | ((((v!(buffer3, 20) | v!(buffer3, 0)) & 214)
                | (v!(buffer3, 20) & v!(buffer3, 0))
                | a)
                & v!(buffer3, 32))
    );
    let source_a = v!(buffer2, index(v!(buffer1, 57), 35));
    let source_b = v!(buffer0, index(v!(buffer3, 64), 20));
    b = ((source_a & source_b) | ((source_b | source_a) & 95) | (v!(buffer3, 64) & 45) | 82) & 32;
    c = ((source_a & source_b) | ((source_a | source_b) & 95)) & ((v!(buffer3, 64) & 45) | 82);
    d = (((v!(buffer3, 0) / 3) - (v!(buffer3, 64) | v!(buffer1, 22))) ^ (v!(buffer3, 28) + 62))
        ^ (b | c);
    t = v!(buffer0, index(d & 0xff, 20));
    put!(
        buffer3,
        68,
        (v!(buffer0, index(v!(buffer1, 99), 20))
            * v!(buffer0, index(v!(buffer1, 99), 20))
            * v!(buffer0, index(v!(buffer1, 99), 20))
            * v!(buffer0, index(v!(buffer1, 99), 20)))
            | v!(buffer2, index(v!(buffer3, 64), 35))
    );
    u = v!(buffer0, index(v!(buffer1, 50), 20));
    w = v!(buffer2, index(v!(buffer1, 138), 35));
    x = v!(buffer4, index(v!(buffer1, 39), 21));
    y = v!(buffer0, index(v!(buffer1, 4), 20));
    z = v!(buffer4, index(v!(buffer1, 202), 21));
    v = v!(buffer0, index(v!(buffer1, 151), 20));
    s = v!(buffer2, index(v!(buffer1, 14), 35));
    r = v!(buffer0, index(v!(buffer1, 145), 20));
    a = (v!(buffer2, index(v!(buffer3, 68), 35)) & v!(buffer0, index(v!(buffer1, 209), 20)))
        | ((v!(buffer2, index(v!(buffer3, 68), 35)) | v!(buffer0, index(v!(buffer1, 209), 20)))
            & 24);
    b = CUnsigned(weird_rol8(
        v!(buffer4, index(v!(buffer1, 127), 21)).0 as u8,
        (v!(buffer2, index(v!(buffer3, 68), 35)) & 7).0 as i32,
    ));
    c = (a & v!(buffer0, 10)) | (b & !v!(buffer0, 10));
    d = CUnsigned(7) ^ (v!(buffer4, index(v!(buffer2, index(v!(buffer3, 36), 35)), 21)) << 1);
    put!(buffer3, 72, (c & 71) | (d & !71));
    add!(
        buffer2,
        2,
        (((v!(buffer0, index(v!(buffer3, 20), 20)) << 1) & 159)
            | (v!(buffer4, index(v!(buffer1, 190), 21)) & !159))
            & ((((v!(buffer4, index(v!(buffer3, 64), 21)) & 110)
                | (v!(buffer0, index(v!(buffer1, 25), 20)) & !110))
                & !150)
                | (v!(buffer1, 25) & 150))
    );
    sub!(
        buffer2,
        14,
        (v!(buffer2, index(v!(buffer3, 20), 35))
            & (v!(buffer3, 72) ^ v!(buffer2, index(v!(buffer1, 100), 35)))
            & !34)
            | (v!(buffer1, 97) & 34),
    );
    put!(buffer0, 17, 115);
    let source_a = v!(buffer4, index(v!(buffer1, 17), 21));
    let source_b = v!(buffer0, index(v!(buffer3, 20), 20));
    let mixed = ((source_a | source_b) & v!(buffer3, 72)) | (source_a & source_b);
    let first = mixed & (v!(buffer1, 50) / 3);
    let second = (mixed | (v!(buffer1, 50) / 3)) & 246;
    xor!(buffer1, 23, (first | second) << 1);
    let first_value = v!(buffer0, index(v!(buffer3, 40), 20));
    let mixed = ((first_value | v!(buffer1, 10)) & 82) | (first_value & v!(buffer1, 10));
    let pre_shift = (mixed & 209) | ((v!(buffer0, index(v!(buffer1, 39), 20)) << 1) & 46);
    put!(buffer0, 13, pre_shift >> 1);
    sub!(buffer2, 33, v!(buffer1, 113) & 9);
    sub!(
        buffer2,
        28,
        ((((CUnsigned(2) | (v!(buffer1, 110) & 222)) >> 1) & !223) | (v!(buffer3, 20) & 223))
    );
    j = CUnsigned(weird_rol8(v.0 as u8 | z.0 as u8, (u & 7).0 as i32));
    a = (v!(buffer2, 16) & t) | (w & !v!(buffer2, 16));
    b = (v!(buffer1, 33) & 17) | (x & !17);
    e = ((y | ((a + b) / 5)) & 147) | (y & ((a + b) / 5));
    m = (v!(buffer3, 40) & v!(buffer4, index((v!(buffer3, 8) + j + e) & 0xff, 21)))
        | ((v!(buffer3, 40) | v!(buffer4, index((v!(buffer3, 8) + j + e) & 0xff, 21)))
            & v!(buffer2, 23));
    put!(
        buffer0,
        15,
        (((v!(buffer4, index(v!(buffer3, 20), 21)) - 48) & (!v!(buffer1, 184)))
            | ((v!(buffer4, index(v!(buffer3, 20), 21)) - 48) & 189)
            | (CUnsigned(189) & !v!(buffer1, 184)))
            & (m * m * m)
    );
    add!(buffer2, 22, v!(buffer1, 183));
    put!(
        buffer3,
        76,
        (CUnsigned(3) * v!(buffer4, index(v!(buffer1, 1), 21))) ^ v!(buffer3, 0)
    );
    a = v!(buffer2, index((v!(buffer3, 8) + (j + e)) & 0xff, 35));
    f = (((v!(buffer4, index(v!(buffer1, 178), 21)) & a)
        | ((v!(buffer4, index(v!(buffer1, 178), 21)) | a) & 209))
        * v!(buffer0, index(v!(buffer1, 13), 20)))
        * (v!(buffer4, index(v!(buffer1, 26), 21)) >> 1);
    g = (f + 0x733f_fff9) * 198 - (((f + 0x733f_fff9) * 396 + 212) & 212) + 85;
    put!(
        buffer3,
        80,
        v!(buffer3, 36) + (g ^ 148) + ((g ^ 107) << 1) - 127
    );
    put!(
        buffer3,
        84,
        (v!(buffer2, index(v!(buffer3, 64), 35)) & 245)
            | (v!(buffer2, index(v!(buffer3, 20), 35)) & 10)
    );
    a = v!(buffer0, index(v!(buffer3, 68), 20)) | 81;
    sub!(
        buffer2,
        18,
        ((a * a * a) & !v!(buffer0, 15)) | ((v!(buffer3, 80) / 15) & v!(buffer0, 15))
    );
    let wrapped_index = index((v!(buffer3, 8) + j + e) & 255, 20);
    let nested_index = index(v!(buffer0, wrapped_index), 21);
    put!(
        buffer3,
        88,
        v!(buffer3, 8) + j + e - v!(buffer0, index(v!(buffer1, 160), 20))
            + (v!(buffer4, nested_index) / 3),
    );
    b = (r ^ v!(buffer3, 72)) & !198 | ((s * s) & 198);
    f = (v!(buffer4, index(v!(buffer1, 69), 21)) & v!(buffer1, 172))
        | ((v!(buffer4, index(v!(buffer1, 69), 21)) | v!(buffer1, 172))
            & ((v!(buffer3, 12) - b) + 77));
    put!(
        buffer0,
        16,
        CUnsigned(147)
            - ((v!(buffer3, 72) & ((f & 251) | 1)) | (((f & 250) | v!(buffer3, 72)) & 198))
    );
    c = (v!(buffer4, index(v!(buffer1, 168), 21)) & v!(buffer0, index(v!(buffer1, 29), 20)) & 7)
        | ((v!(buffer4, index(v!(buffer1, 168), 21)) | v!(buffer0, index(v!(buffer1, 29), 20)))
            & 6);
    f = (v!(buffer4, index(v!(buffer1, 155), 21)) & v!(buffer1, 105))
        | ((v!(buffer4, index(v!(buffer1, 155), 21)) | v!(buffer1, 105)) & 141);
    sub!(
        buffer0,
        3,
        v!(
            buffer4,
            index(CUnsigned(weird_rol32(f.0 as u8, c.0 as i32)), 21)
        )
    );
    put!(
        buffer1,
        5,
        CUnsigned(weird_ror8(
            v!(buffer0, 12).0 as u8,
            ((v!(buffer0, index(v!(buffer1, 61), 20)) / 5) & 7).0 as i32
        )) ^ (!v!(buffer2, index(v!(buffer3, 84), 35)) / 5)
    );
    add!(buffer1, 198, v!(buffer1, 3));
    a = CUnsigned(162) | v!(buffer2, index(v!(buffer3, 64), 35));
    add!(buffer1, 164, (a * a) / 5);
    g = CUnsigned(weird_ror8(139, (v!(buffer3, 80) & 7).0 as i32));
    c = ((v!(buffer4, index(v!(buffer3, 64), 21))
        * v!(buffer4, index(v!(buffer3, 64), 21))
        * v!(buffer4, index(v!(buffer3, 64), 21)))
        & 95)
        | (v!(buffer0, index(v!(buffer3, 40), 20)) & !95);
    put!(
        buffer3,
        92,
        (g & 12)
            | (v!(buffer0, index(v!(buffer3, 20), 20)) & 12)
            | (g & v!(buffer0, index(v!(buffer3, 20), 20)))
            | c
    );
    add!(
        buffer2,
        12,
        ((v!(buffer1, 103) & 32) | (v!(buffer3, 92) & (v!(buffer1, 103) | 60)) | 16) / 3
    );
    put!(buffer3, 96, v!(buffer1, 143));
    put!(buffer3, 100, 27);
    put!(
        buffer3,
        104,
        (((v!(buffer3, 40) & !v!(buffer2, 8)) | (v!(buffer1, 35) & v!(buffer2, 8)))
            & v!(buffer3, 64))
            ^ 119
    );
    put!(
        buffer3,
        108,
        CUnsigned(238)
            & ((((v!(buffer3, 40) & !v!(buffer2, 8)) | (v!(buffer1, 35) & v!(buffer2, 8)))
                & v!(buffer3, 64))
                << 1)
    );
    put!(
        buffer3,
        112,
        (!v!(buffer3, 64) & (v!(buffer3, 84) / 3)) ^ 49
    );
    put!(
        buffer3,
        116,
        CUnsigned(98) & ((!v!(buffer3, 64) & (v!(buffer3, 84) / 3)) << 1)
    );
    a = (v!(buffer1, 35) & v!(buffer2, 8)) | (v!(buffer3, 40) & !v!(buffer2, 8));
    b = (a & v!(buffer3, 64)) | ((v!(buffer3, 84) / 3) & !v!(buffer3, 64));
    let term1 = b & (CUnsigned(86) + ((v!(buffer1, 172) & 64) >> 1));
    let term2 = (((v!(buffer1, 172) & 65) >> 1) ^ 86)
        | ((!v!(buffer3, 64) & (v!(buffer3, 84) / 3))
            | (((v!(buffer3, 40) & !v!(buffer2, 8)) | (v!(buffer1, 35) & v!(buffer2, 8)))
                & v!(buffer3, 64)));
    let term2 = term2 & v!(buffer3, 100);
    put!(buffer1, 143, v!(buffer3, 96) - (term1 | term2));
    put!(buffer2, 29, 162);
    a = ((v!(buffer4, index(v!(buffer3, 88), 21)) & 160)
        | (v!(buffer0, index(v!(buffer1, 125), 20)) & 95))
        >> 1;
    b = v!(buffer2, index(v!(buffer1, 149), 35)) ^ (v!(buffer1, 43) * v!(buffer1, 43));
    add!(buffer0, 15, (b & a) | ((a | b) & 115));
    put!(
        buffer3,
        120,
        v!(buffer3, 64) - v!(buffer0, index(v!(buffer3, 40), 20))
    );
    put!(buffer1, 95, v!(buffer4, index(v!(buffer3, 20), 21)));
    a = CUnsigned(weird_ror8(
        v!(buffer2, index(v!(buffer3, 80), 35)).0 as u8,
        ((v!(buffer2, index(v!(buffer1, 17), 35))
            * v!(buffer2, index(v!(buffer1, 17), 35))
            * v!(buffer2, index(v!(buffer1, 17), 35)))
            & 7)
        .0 as i32,
    ));
    sub!(buffer0, 7, a * a);
    put!(
        buffer2,
        8,
        v!(buffer2, 8) - v!(buffer1, 184)
            + (v!(buffer4, index(v!(buffer1, 202), 21))
                * v!(buffer4, index(v!(buffer1, 202), 21))
                * v!(buffer4, index(v!(buffer1, 202), 21)))
    );
    put!(
        buffer0,
        16,
        (v!(buffer2, index(v!(buffer1, 102), 35)) << 1) & 132
    );
    put!(
        buffer3,
        124,
        (v!(buffer4, index(v!(buffer3, 40), 21)) >> 1) ^ v!(buffer3, 68)
    );
    sub!(
        buffer0,
        7,
        v!(buffer0, index(v!(buffer1, 191), 20))
            - (((v!(buffer4, index(v!(buffer1, 80), 21)) << 1) & !177)
                | (v!(buffer4, index(v!(buffer4, index(v!(buffer3, 88), 21)), 21)) & 177))
    );
    put!(buffer0, 6, v!(buffer0, index(v!(buffer1, 119), 20)));
    a = (v!(buffer4, index(v!(buffer1, 190), 21)) & !209) | (v!(buffer1, 118) & 209);
    b = v!(buffer0, index(v!(buffer3, 120), 20)) * v!(buffer0, index(v!(buffer3, 120), 20));
    put!(
        buffer0,
        12,
        (v!(buffer0, index(v!(buffer3, 84), 20))
            ^ (v!(buffer2, index(v!(buffer1, 71), 35)) + v!(buffer2, index(v!(buffer1, 15), 35))))
            & ((a & b) | ((a | b) & 27)),
    );
    b = (v!(buffer1, 32) & v!(buffer2, index(v!(buffer3, 88), 35)))
        | ((v!(buffer1, 32) | v!(buffer2, index(v!(buffer3, 88), 35))) & 23);
    d = ((v!(buffer4, index(v!(buffer1, 57), 21)) * 231) & 169) | (b & 86);
    f = (((v!(buffer0, index(v!(buffer1, 82), 20)) & !29)
        | (v!(buffer4, index(v!(buffer3, 124), 21)) & 29))
        & 190)
        | (v!(buffer4, index(d / 5, 21)) & !190);
    h = v!(buffer0, index(v!(buffer3, 40), 20))
        * v!(buffer0, index(v!(buffer3, 40), 20))
        * v!(buffer0, index(v!(buffer3, 40), 20));
    k = (h & v!(buffer1, 82)) | (h & 92) | (v!(buffer1, 82) & 92);
    put!(buffer3, 128, ((f & k) | ((f | k) & 192)) ^ (d / 5));
    xor!(
        buffer2,
        25,
        ((v!(buffer0, index(v!(buffer3, 120), 20)) << 1) * v!(buffer1, 5))
            - (CUnsigned(weird_rol8(
                v!(buffer3, 76).0 as u8,
                (v!(buffer4, index(v!(buffer3, 124), 21)) & 7).0 as i32,
            )) & (v!(buffer3, 20) + 110)),
    );
}

#[cfg(test)]
mod tests {
    use super::{CUnsigned, garble, weird_rol8, weird_rol32, weird_ror8};

    fn hex(bytes: &[u8]) -> String {
        const DIGITS: &[u8; 16] = b"0123456789abcdef";
        let mut text = String::with_capacity(bytes.len() * 2);
        for byte in bytes {
            text.push(DIGITS[(byte >> 4) as usize] as char);
            text.push(DIGITS[(byte & 0x0f) as usize] as char);
        }
        text
    }

    #[test]
    fn unsigned_arithmetic_wraps_with_overflow_checks_enabled() {
        assert_eq!((CUnsigned(0) - 1).0, u32::MAX);
        assert_eq!((CUnsigned(u32::MAX) + 1).0, 0);
        assert_eq!((CUnsigned(u32::MAX) * 2).0, u32::MAX - 1);
    }

    #[test]
    fn garble_matches_the_c_reference_for_a_deterministic_buffer_set() {
        let mut buffer0 = std::array::from_fn(|i| (i * 17 + 5) as u8);
        let mut buffer1 = std::array::from_fn(|i| (i * 13 + 7) as u8);
        let mut buffer2 = std::array::from_fn(|i| (i * 11 + 9) as u8);
        let mut buffer3 = std::array::from_fn(|i| (i * 7 + 11) as u8);
        let buffer4 = std::array::from_fn(|i| (i * 5 + 13) as u8);

        garble(
            &mut buffer0,
            &mut buffer1,
            &mut buffer2,
            &mut buffer3,
            &buffer4,
        );
        assert_eq!(
            hex(&buffer1),
            "0714e12e08fd55626f7c8996a3b0bddbd7e4f1220b1855da3f4c596673808d9aa782d6ce58e8f5020f1c293643505d677784919eabd0c5d2dfecf90613202d3a4754616e7b8895a2afbcc9d6e3f0fd0a1724313e4b5865727f8c99a6b3c0cd30e7f4010e1b2835424f5cf07683909daab7c4d1deebf805121f2c399bb5606d7a8794a1aebbc8d5e2effc091623303d385764717e8b98a5b2bf51d9e6f3000d1a2734414e1b6875828f9ca9b6c3d0ddeaf704111e2b3845525f6c798693a038bac7d4e1eefb0843222f3c495663707d8a97a4"
        );
        assert_eq!(
            hex(&buffer2),
            "094b272a2dfc4b56426c7782d2957baec4c446dae5f024061128273221a2df5e692db8"
        );
        assert_eq!(
            hex(&buffer3),
            "2a121920d02e353c404a51589b666d74ae8289901c9ea5acb2bac1c8cfd6dde40df2f900390e151c5a2a31383c464d54106269701b7e858cc79aa1a8afb6bdc484d2d9e0f3eef5fc580a111879262d344f424950305e656c197a81881f969da44ab2b9c01bced5dcf7eaf1f800060d1421222930203e454c765a6168e6767d84969299a0"
        );
        assert_eq!(hex(&buffer4), "0d12171c21262b30353a3f44494e53585d62676c71");
        assert_eq!(hex(&buffer0), "051627a4495a37208d9e0ec011075d56847337cd");
    }

    #[test]
    fn zero_count_is_a_special_case_in_the_c_helpers() {
        assert_eq!(weird_ror8(0xff, 0), 0);
        assert_eq!(weird_rol8(0xff, 0), 0);
        assert_eq!(weird_rol32(0xff, 0), 0);
    }

    #[test]
    fn byte_helpers_keep_the_c_expression_width() {
        assert_eq!(weird_ror8(0x80, 1), 0x4040);
        assert_eq!(weird_rol8(0x80, 1), 0x01);
        assert_eq!(weird_rol32(0x80, 1), 0x101);
        assert_eq!(weird_ror8(0xff, 8), 0xff);
        assert_eq!(weird_rol8(0xff, 8), 0xff);
        assert_eq!(weird_rol32(0xff, 8), 0xff00 ^ 0xff);
    }

    #[test]
    fn invalid_counts_do_not_panic_for_undefined_c_inputs() {
        assert_eq!(weird_ror8(0xff, -1), 0);
        assert_eq!(weird_rol8(0xff, 9), 0);
        assert_eq!(weird_rol32(0xff, 32), 0);
    }
}
