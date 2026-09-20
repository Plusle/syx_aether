#![allow(dead_code)]

use std::fmt;

use bitflags::Bits;

pub fn format_flags<T: bitflags::Flags>(flags: &T, f: &mut fmt::Formatter<'_>) -> fmt::Result
where
    T::Bits: fmt::LowerHex,
{
    let mut first = true;

    for flag in flags.iter_names() {
        if !first {
            write!(f, " | ")?;
        }
        write!(f, "{}", flag.0)?;
        first = false;
    }

    let unknown = flags.bits() & !T::all().bits();
    if unknown != T::Bits::EMPTY {
        if !first {
            write!(f, " | ")?;
        }
        write!(f, "UNKNOWN 0x{:x}", unknown)?;
        first = false;
    }

    if first {
        write!(f, "(empty)")?;
    }

    Ok(())
}
