use std::iter::FromIterator;

use crate::descriptor::Error;

const INPUT_CHARSET: &str =  "0123456789()[],'/*abcdefgh@:$%{}IJKLMNOPQRSTUVWXYZ&+-.;<=>?!^_|~ijklmnopqrstuvwxyzABCDEFGH`#\"\\ ";
const CHECKSUM_CHARSET: &str = "qpzry9x8gf2tvdw0s3jn54khce6mua7l";

fn poly_mod(mut c: u64, val: u64) -> u64 {
    let c0 = c >> 35;
    c = ((c & 0x7ffffffff) << 5) ^ val;
    if c0 & 1 > 0 {
        c ^= 0xf5dee51989
    };
    if c0 & 2 > 0 {
        c ^= 0xa9fdca3312
    };
    if c0 & 4 > 0 {
        c ^= 0x1bab10e32d
    };
    if c0 & 8 > 0 {
        c ^= 0x3706b1677a
    };
    if c0 & 16 > 0 {
        c ^= 0x644d626ffd
    };

    c
}

pub fn get_checksum(desc: &str) -> Result<String, Error> {
    let mut c = 1;
    let mut cls = 0;
    let mut clscount = 0;
    for ch in desc.chars() {
        let pos = INPUT_CHARSET
            .find(ch)
            .ok_or(Error::InvalidDescriptorCharacter(ch))? as u64;
        c = poly_mod(c, pos & 31);
        cls = cls * 3 + (pos >> 5);
        clscount += 1;
        if clscount == 3 {
            c = poly_mod(c, cls);
            cls = 0;
            clscount = 0;
        }
    }
    if clscount > 0 {
        c = poly_mod(c, cls);
    }
    (0..8).for_each(|_| c = poly_mod(c, 0));
    c ^= 1;

    let mut chars = Vec::with_capacity(8);
    for j in 0..8 {
        chars.push(
            CHECKSUM_CHARSET
                .chars()
                .nth(((c >> (5 * (7 - j))) & 31) as usize)
                .unwrap(),
        );
    }

    Ok(String::from_iter(chars))
}
