//! Wyhash as implemented by Zig 0.16 `std.hash.Wyhash` (final v4.1 variant). Kata stores
//! `Wyhash(seed 0, plan bytes)` in state files, so the value must match the Zig build exactly.

const SECRET: [u64; 4] = [
    0xa076_1d64_78bd_642f,
    0xe703_7ed1_a0b4_28db,
    0x8ebc_6af0_9c88_c6e3,
    0x5899_65cc_7537_4cc3,
];

fn read(bytes: usize, data: &[u8]) -> u64 {
    let mut value = 0u64;
    for (i, byte) in data[..bytes].iter().enumerate() {
        value |= (*byte as u64) << (8 * i);
    }
    value
}

fn mum(a: u64, b: u64) -> (u64, u64) {
    let x = (a as u128).wrapping_mul(b as u128);
    (x as u64, (x >> 64) as u64)
}

fn mix(a: u64, b: u64) -> u64 {
    let (lo, hi) = mum(a, b);
    lo ^ hi
}

/// One-shot hash of `input` with `seed`.
pub fn hash(seed: u64, input: &[u8]) -> u64 {
    let mut state = [0u64; 3];
    state[0] = seed ^ mix(seed ^ SECRET[0], SECRET[1]);
    state[1] = state[0];
    state[2] = state[0];

    let (mut a, mut b);
    if input.len() <= 16 {
        if input.len() >= 4 {
            let end = input.len() - 4;
            let quarter = (input.len() >> 3) << 2;
            a = (read(4, input) << 32) | read(4, &input[quarter..]);
            b = (read(4, &input[end..]) << 32) | read(4, &input[end - quarter..]);
        } else if !input.is_empty() {
            a = ((input[0] as u64) << 16)
                | ((input[input.len() >> 1] as u64) << 8)
                | input[input.len() - 1] as u64;
            b = 0;
        } else {
            a = 0;
            b = 0;
        }
    } else {
        let mut i = 0;
        if input.len() >= 48 {
            while i + 48 < input.len() {
                for lane in 0..3 {
                    let x = read(8, &input[i + 16 * lane..]);
                    let y = read(8, &input[i + 16 * lane + 8..]);
                    state[lane] = mix(x ^ SECRET[lane + 1], y ^ state[lane]);
                }
                i += 48;
            }
            state[0] ^= state[1] ^ state[2];
        }
        let tail = &input[i..];
        let mut j = 0;
        while j + 16 < tail.len() {
            state[0] = mix(
                read(8, &tail[j..]) ^ SECRET[1],
                read(8, &tail[j + 8..]) ^ state[0],
            );
            j += 16;
        }
        a = read(8, &input[input.len() - 16..]);
        b = read(8, &input[input.len() - 8..]);
    }

    a ^= SECRET[1];
    b ^= state[0];
    let (lo, hi) = mum(a, b);
    mix(lo ^ SECRET[0] ^ input.len() as u64, hi ^ SECRET[1])
}

#[cfg(test)]
mod tests {
    use super::hash;

    // Vectors from the wyhash reference test_vector.cpp, as listed in Zig's std.
    #[test]
    fn matches_reference_vectors() {
        let vectors: [(u64, u64, &str); 7] = [
            (0, 0x0409_638e_e2bd_e459, ""),
            (1, 0xa841_2d09_1b5f_e0a9, "a"),
            (2, 0x32dd_92e4_b291_5153, "abc"),
            (3, 0x8619_1240_89a3_a16b, "message digest"),
            (4, 0x7a43_afb6_1d7f_5f40, "abcdefghijklmnopqrstuvwxyz"),
            (
                5,
                0xff42_329b_90e5_0d58,
                "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789",
            ),
            (
                6,
                0xc39c_ab13_b115_aad3,
                "12345678901234567890123456789012345678901234567890123456789012345678901234567890",
            ),
        ];
        for (seed, expected, input) in vectors {
            assert_eq!(hash(seed, input.as_bytes()), expected, "{input:?}");
        }
    }
}
