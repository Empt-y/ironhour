//! Field values: the original's value parsers, ported one by one.
//!
//! Each C++ parse procedure (`INI::parseReal`, `KindOfMaskType::parseFromINI`, ...)
//! maps to a function here that consumes tokens the same way (same separator sets,
//! same `strtok` behaviour) and applies the same unit conversions. Sources:
//! `GameEngine/Source/Common/INI/INI.cpp`, `Include/Common/BitFlagsIO.h`,
//! `Include/Common/GameCommon.h`, `Source/Common/GameLOD.cpp`.
//!
//! Template references (weapons, FX lists, ...) are kept as names; resolving them
//! needs the whole data set and happens in a later pass.

use crate::lexer::{SEPS, SEPS_COLON, SEPS_PERCENT, SEPS_QUOTE, Tokens, text};
use crate::schema::{FieldDef, schema};

/// `LOGICFRAMES_PER_SECOND` (`GameCommon.h`).
pub const LOGIC_FPS: f32 = 30.0;
const LOGICFRAMES_PER_MSEC: f32 = 30.0 / 1000.0;
const SECONDS_PER_LOGICFRAME: f32 = 1.0 / LOGIC_FPS;
/// `#define PI 3.14159265359f` (`BaseType.h`) rounds to the same f32 as `consts::PI`.
const PI: f32 = std::f32::consts::PI;
const RADS_PER_DEGREE: f32 = PI / 180.0;

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Bool(bool),
    Int(i32),
    UInt(u32),
    Real(f32),
    Str(String),
    Strings(Vec<String>),
    /// A name from a fixed list, with its index (or lookup value) in that list.
    Index {
        name: &'static str,
        index: i32,
    },
    Bits(Bits),
    Coord2([f32; 2]),
    Coord3([f32; 3]),
    ICoord2([i32; 2]),
    /// `parseRGBColor`: components 0..=1.
    Rgb([f32; 3]),
    /// `parseRGBAColorInt` / `parseColorInt`: components 0..=255.
    Rgba([u8; 4]),
    /// A reference to another definition by name; `None` for `None`/`NoSound`.
    Ref {
        kind: RefKind,
        name: Option<String>,
    },
    /// `parseGameClientRandomVariable`: low, high, distribution index.
    Random {
        low: f32,
        high: f32,
        distribution: i32,
    },
    /// Several values from one line (e.g. a weapon slot and a weapon name).
    Tuple(Vec<Value>),
}

/// A bit-set value. `replace` sets start from empty; otherwise the listed changes
/// apply to the field's previous value (inherited defaults, earlier lines).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Bits {
    pub replace: bool,
    pub set: Vec<u32>,
    pub clear: Vec<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum RefKind {
    Object,
    Armor,
    Weapon,
    FxList,
    ParticleSystem,
    DamageFx,
    ObjectCreationList,
    Upgrade,
    SpecialPower,
    Science,
    Audio,
    MappedImage,
    /// A string-table label (`parseAndTranslateLabel`).
    Label,
    CommandButton,
    Locomotor,
    Crate,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ValueError {
    /// The original would throw while parsing this value.
    Invalid(String),
    /// This C++ parser has not been ported yet.
    Unimplemented(&'static str),
}

type R<T> = Result<T, ValueError>;

fn invalid<T>(msg: impl Into<String>) -> R<T> {
    Err(ValueError::Invalid(msg.into()))
}

/// Token reader over a field's raw text with the original's error behaviour.
struct In<'a> {
    t: Tokens<'a>,
}

impl<'a> In<'a> {
    fn next_with(&mut self, seps: &[u8]) -> R<&'a [u8]> {
        match self.t.next_with(seps) {
            Some(t) => Ok(t),
            None => invalid("missing value"), // getNextToken throws INI_INVALID_DATA
        }
    }
    fn next(&mut self) -> R<&'a [u8]> {
        self.next_with(SEPS)
    }
    fn next_or_none(&mut self) -> Option<&'a [u8]> {
        self.t.next_token()
    }
    fn next_or_none_with(&mut self, seps: &[u8]) -> Option<&'a [u8]> {
        self.t.next_with(seps)
    }
    /// `getNextSubToken`: expects `label:value` (colon separators) and returns value.
    fn sub(&mut self, label: &str) -> R<&'a [u8]> {
        let l = self.next_with(SEPS_COLON)?;
        if !l.eq_ignore_ascii_case(label.as_bytes()) {
            return invalid(format!("expected '{label}:', found '{}'", text(l)));
        }
        self.next_with(SEPS_COLON)
    }
}

/// `sscanf("%d")`: optional sign and leading digits; trailing text is ignored.
pub fn scan_int(tok: &[u8]) -> R<i32> {
    let (neg, digits) = sign_and_digits(tok);
    if digits.is_empty() {
        return invalid(format!("'{}' is not an integer", text(tok)));
    }
    let mut v: i64 = 0;
    for &d in digits {
        v = v.wrapping_mul(10).wrapping_add(i64::from(d - b'0'));
    }
    Ok((if neg { v.wrapping_neg() } else { v }) as i32)
}

/// `sscanf("%u")`: like `%d` but negative input wraps, as MSVC's does.
pub fn scan_uint(tok: &[u8]) -> R<u32> {
    scan_int(tok).map(|v| v as u32)
}

fn sign_and_digits(tok: &[u8]) -> (bool, &[u8]) {
    let (neg, rest) = match tok.first() {
        Some(b'-') => (true, &tok[1..]),
        Some(b'+') => (false, &tok[1..]),
        _ => (false, tok),
    };
    let n = rest.iter().take_while(|c| c.is_ascii_digit()).count();
    (neg, &rest[..n])
}

/// `sscanf("%f")`: the longest valid float prefix.
pub fn scan_real(tok: &[u8]) -> R<f32> {
    let mut i = 0;
    if matches!(tok.first(), Some(b'+' | b'-')) {
        i = 1;
    }
    let int_digits = tok[i..].iter().take_while(|c| c.is_ascii_digit()).count();
    i += int_digits;
    let mut frac_digits = 0;
    if tok.get(i) == Some(&b'.') {
        frac_digits = tok[i + 1..].iter().take_while(|c| c.is_ascii_digit()).count();
        i += 1 + frac_digits;
    }
    if int_digits + frac_digits == 0 {
        return invalid(format!("'{}' is not a number", text(tok)));
    }
    if matches!(tok.get(i), Some(b'e' | b'E')) {
        let mut j = i + 1;
        if matches!(tok.get(j), Some(b'+' | b'-')) {
            j += 1;
        }
        let exp_digits = tok[j..].iter().take_while(|c| c.is_ascii_digit()).count();
        if exp_digits > 0 {
            i = j + exp_digits;
        }
    }
    let s = std::str::from_utf8(&tok[..i]).unwrap_or("0");
    let s = s.strip_suffix('.').unwrap_or(s);
    s.parse::<f32>().or_else(|_| invalid(format!("'{}' is not a number", text(tok))))
}

/// `INI::scanBool`: `yes`/`no`, case-insensitive.
pub fn scan_bool(tok: &[u8]) -> R<bool> {
    if tok.eq_ignore_ascii_case(b"yes") {
        Ok(true)
    } else if tok.eq_ignore_ascii_case(b"no") {
        Ok(false)
    } else {
        invalid(format!("'{}' is not Yes or No", text(tok)))
    }
}

/// Strips casts and address-of from a C++ user-data expression, leaving the list name.
fn list_id(user: &str) -> &str {
    let u = user.trim();
    let u = match u.rfind(')') {
        Some(i) if u.starts_with('(') => &u[i + 1..],
        _ => u,
    };
    u.trim().trim_start_matches('&').trim()
}

fn names(list: &str) -> R<&'static [&'static str]> {
    // `SpecialPowerMaskType::getBitNames()` returns the type's s_bitNameList.
    let owned;
    let list = match list.strip_suffix("::getBitNames()") {
        Some(ty) => {
            owned = format!("{ty}::s_bitNameList");
            owned.as_str()
        }
        None => list,
    };
    schema().name_list(list).ok_or_else(|| ValueError::Invalid(format!("name list '{list}' not extracted")))
}

/// `INI::scanIndexList`: case-insensitive position in a name list.
fn scan_index(tok: &[u8], list: &'static [&'static str]) -> R<(i32, &'static str)> {
    match list.iter().position(|n| n.as_bytes().eq_ignore_ascii_case(tok)) {
        Some(i) => Ok((i as i32, list[i])),
        None => invalid(format!("'{}' is not a valid name", text(tok))),
    }
}

/// `parseBitString32` / `BitFlags::parse`: plain names replace, `+X`/`-X` edit.
fn parse_bitstring(inp: &mut In, list: &'static [&'static str]) -> R<Bits> {
    let mut b = Bits::default();
    let (mut normal, mut delta) = (false, false);
    while let Some(tok) = inp.next_or_none() {
        if tok.eq_ignore_ascii_case(b"NONE") {
            if normal || delta {
                return invalid("may not mix NONE with other flags");
            }
            return Ok(Bits { replace: true, ..Bits::default() });
        }
        match tok[0] {
            b'+' | b'-' => {
                if normal {
                    return invalid("may not mix plain and +/- flags");
                }
                let (i, _) = scan_index(&tok[1..], list)?;
                if tok[0] == b'+' {
                    b.set.push(i as u32)
                } else {
                    b.clear.push(i as u32)
                }
                delta = true;
            }
            _ => {
                if delta {
                    return invalid("may not mix plain and +/- flags");
                }
                let (i, _) = scan_index(tok, list)?;
                b.replace = true;
                b.set.push(i as u32);
                normal = true;
            }
        }
    }
    Ok(b)
}

/// Damage/death/veterancy style: starts from ALL; only ALL, NONE, +X and -X.
fn parse_all_none_flags(inp: &mut In, list: &'static [&'static str], start_all: bool) -> R<Bits> {
    let mut set: Vec<bool> = vec![start_all; list.len()];
    let first = inp.next()?;
    let mut tok = Some(first);
    while let Some(t) = tok {
        if t.eq_ignore_ascii_case(b"ALL") {
            set.iter_mut().for_each(|s| *s = true);
        } else if t.eq_ignore_ascii_case(b"NONE") {
            set.iter_mut().for_each(|s| *s = false);
        } else if t[0] == b'+' || t[0] == b'-' {
            let (i, _) = scan_index(&t[1..], list)?;
            set[i as usize] = t[0] == b'+';
        } else {
            return invalid(format!("'{}': expected ALL, NONE, +NAME or -NAME", text(t)));
        }
        tok = inp.next_or_none();
    }
    Ok(Bits { replace: true, set: (0..list.len() as u32).filter(|&i| set[i as usize]).collect(), clear: vec![] })
}

/// `getNextAsciiString`: a token, or a quoted string spanning separators.
fn next_ascii_string(inp: &mut In) -> String {
    let Some(tok) = inp.next_or_none() else { return String::new() };
    if tok.first() != Some(&b'"') {
        return text(tok);
    }
    let mut buf = text(&tok[1..]);
    match inp.next_or_none_with(SEPS_QUOTE) {
        Some(more) => {
            if more.len() > 1 && more[1] != b'\t' {
                buf.push(' ');
            }
            buf.push_str(&text(more));
        }
        None => {
            if buf.ends_with('"') {
                buf.pop();
            }
        }
    }
    buf
}

/// `getNextQuotedAsciiString`.
fn next_quoted_string(inp: &mut In) -> R<String> {
    let Some(tok) = inp.next_or_none() else { return Ok(String::new()) };
    if tok.first() != Some(&b'"') {
        return Ok(text(tok));
    }
    let mut buf = text(&tok[1..]);
    if tok.len() > 1 && buf.ends_with('"') {
        buf.pop();
        return Ok(buf);
    }
    let more = inp.next_with(SEPS_QUOTE)?;
    if more.len() > 1 && more[1] != b'\t' {
        buf.push(' ');
        buf.push_str(&text(more));
    } else if buf.ends_with('"') {
        buf.pop();
    }
    Ok(buf)
}

fn reference(inp: &mut In, kind: RefKind, none_word: &str) -> R<Value> {
    let t = inp.next()?;
    let name = (!t.eq_ignore_ascii_case(none_word.as_bytes())).then(|| text(t));
    Ok(Value::Ref { kind, name })
}

fn rgba(inp: &mut In) -> R<[u8; 4]> {
    let labels = ["R", "G", "B", "A"];
    let mut c = [255u8; 4];
    for (i, l) in labels.iter().enumerate() {
        match inp.next_or_none_with(SEPS_COLON) {
            None if i == 3 => break,
            None => return invalid("missing colour component"),
            Some(t) if !t.eq_ignore_ascii_case(l.as_bytes()) => {
                return invalid(format!("expected '{l}:', found '{}'", text(t)));
            }
            Some(_) => {
                let v = scan_int(inp.next_with(SEPS_COLON)?)?;
                if !(0..=255).contains(&v) {
                    return invalid("colour component out of 0..255");
                }
                c[i] = v as u8;
            }
        }
    }
    Ok(c)
}

fn ranged(v: i32, lo: i32, hi: i32, what: &str) -> R<i32> {
    if v < lo || v > hi { invalid(format!("{v} out of range for {what}")) } else { Ok(v) }
}

/// Parses one field's value with the parser its definition names.
pub fn parse_value(def: &FieldDef, raw: &str) -> R<Value> {
    let bytes: Vec<u8> = raw.chars().map(|c| c as u32 as u8).collect();
    let mut inp = In { t: Tokens::new(&bytes) };
    let inp = &mut inp;
    let p = def.parser;
    let ini = p.strip_prefix("INI::");
    Ok(match ini {
        Some("parseReal") => Value::Real(scan_real(inp.next()?)?),
        Some("parsePositiveNonZeroReal") => {
            let v = scan_real(inp.next()?)?;
            if v <= 0.0 {
                return invalid(format!("{v} must be greater than 0"));
            }
            Value::Real(v)
        }
        Some("parseInt") => Value::Int(scan_int(inp.next()?)?),
        Some("parseUnsignedInt") => Value::UInt(scan_uint(inp.next()?)?),
        Some("parseUnsignedByte") => Value::Int(ranged(scan_int(inp.next()?)?, 0, 255, "a byte")?),
        Some("parseShort") => Value::Int(ranged(scan_int(inp.next()?)?, -32768, 32767, "a short")?),
        Some("parseUnsignedShort") => Value::Int(ranged(scan_int(inp.next()?)?, 0, 65535, "an unsigned short")?),
        Some("parseBool") | Some("parseBitInInt32") => Value::Bool(scan_bool(inp.next()?)?),
        Some("parseAngleReal") => Value::Real(scan_real(inp.next()?)? * RADS_PER_DEGREE),
        Some("parseAngularVelocityReal") => {
            Value::Real(scan_real(inp.next()?)? * (SECONDS_PER_LOGICFRAME * RADS_PER_DEGREE))
        }
        Some("parsePercentToReal") => Value::Real(scan_real(inp.next_with(SEPS_PERCENT)?)? / 100.0),
        Some("parseDurationReal") => Value::Real(scan_real(inp.next()?)? * LOGICFRAMES_PER_MSEC),
        Some("parseDurationUnsignedInt") => {
            Value::UInt((scan_uint(inp.next()?)? as f32 * LOGICFRAMES_PER_MSEC).ceil() as u32)
        }
        Some("parseDurationUnsignedShort") => {
            Value::UInt(u32::from((scan_uint(inp.next()?)? as f32 * LOGICFRAMES_PER_MSEC).ceil() as u16))
        }
        Some("parseVelocityReal") => Value::Real(scan_real(inp.next()?)? * SECONDS_PER_LOGICFRAME),
        Some("parseAccelerationReal") => {
            Value::Real(scan_real(inp.next()?)? * (SECONDS_PER_LOGICFRAME * SECONDS_PER_LOGICFRAME))
        }
        Some("parseAsciiString") => Value::Str(next_ascii_string(inp)),
        Some("parseQuotedAsciiString") => Value::Str(next_quoted_string(inp)?),
        Some("parseAsciiStringVector") | Some("parseAsciiStringVectorAppend") => {
            Value::Strings(std::iter::from_fn(|| inp.next_or_none()).map(text).collect())
        }
        Some("parseSoundsList") => {
            Value::Strings(std::iter::from_fn(|| inp.next_or_none_with(b" \t,=")).map(text).collect())
        }
        Some("parseScienceVector") => {
            let mut v: Vec<String> = Vec::new();
            while let Some(t) = inp.next_or_none() {
                if t.eq_ignore_ascii_case(b"None") {
                    v.clear();
                    break;
                }
                v.push(text(t));
            }
            Value::Strings(v)
        }
        Some("parseRGBColor") => {
            let mut c = [0f32; 3];
            for (i, l) in ["R", "G", "B"].iter().enumerate() {
                let v = ranged(scan_int(inp.sub(l)?)?, 0, 255, "a colour component")?;
                c[i] = v as f32 / 255.0;
            }
            Value::Rgb(c)
        }
        Some("parseRGBAColorInt") | Some("parseColorInt") => Value::Rgba(rgba(inp)?),
        Some("parseCoord3D") => {
            Value::Coord3([scan_real(inp.sub("X")?)?, scan_real(inp.sub("Y")?)?, scan_real(inp.sub("Z")?)?])
        }
        Some("parseCoord2D") => Value::Coord2([scan_real(inp.sub("X")?)?, scan_real(inp.sub("Y")?)?]),
        Some("parseICoord2D") => Value::ICoord2([scan_int(inp.sub("X")?)?, scan_int(inp.sub("Y")?)?]),
        Some("parseIndexList") | Some("parseByteSizedIndexList") => {
            let (index, name) = scan_index(inp.next()?, names(list_id(def.user))?)?;
            if p.ends_with("ByteSizedIndexList") {
                ranged(index, 0, 255, "a byte index")?;
            }
            Value::Index { name, index }
        }
        Some("parseLookupList") => {
            let id = list_id(def.user);
            let list = schema()
                .lookup_list(id)
                .ok_or_else(|| ValueError::Invalid(format!("lookup list '{id}' not extracted")))?;
            let t = inp.next()?;
            match list.iter().position(|(n, _)| n.as_bytes().eq_ignore_ascii_case(t)) {
                Some(i) => Value::Index { name: list[i].0, index: i as i32 },
                None => return invalid(format!("'{}' is not a valid name", text(t))),
            }
        }
        Some("parseBitString32") | Some("parseBitString8") => {
            let b = parse_bitstring(inp, names(list_id(def.user))?)?;
            if p.ends_with('8') && b.set.iter().chain(&b.clear).any(|&i| i >= 8) {
                return invalid("flag does not fit in 8 bits");
            }
            Value::Bits(b)
        }
        Some("parseDamageTypeFlags") => {
            Value::Bits(parse_all_none_flags(inp, names("DamageTypeFlags::s_bitNameList")?, true)?)
        }
        Some("parseDeathTypeFlags") => Value::Bits(parse_all_none_flags(inp, names("TheDeathNames")?, true)?),
        Some("parseVeterancyLevelFlags") => Value::Bits(parse_all_none_flags(inp, names("TheVeterancyNames")?, true)?),
        Some("parseGameClientRandomVariable") => {
            let low = scan_real(inp.next()?)?;
            let high = scan_real(inp.next()?)?;
            let distribution = match inp.next_or_none() {
                Some(t) => scan_index(t, names("GameClientRandomVariable::DistributionTypeNames")?)?.0,
                None => 0,
            };
            Value::Random { low, high, distribution }
        }
        Some("parseStaticGameLODLevel") => {
            let (index, name) = scan_index(inp.next()?, names("StaticGameLODNames")?)?;
            Value::Index { name, index }
        }
        Some("parseThingTemplate") => reference(inp, RefKind::Object, "None")?,
        Some("parseArmorTemplate") => reference(inp, RefKind::Armor, "None")?,
        Some("parseWeaponTemplate") => reference(inp, RefKind::Weapon, "None")?,
        Some("parseFXList") => reference(inp, RefKind::FxList, "None")?,
        Some("parseParticleSystemTemplate") => reference(inp, RefKind::ParticleSystem, "None")?,
        Some("parseDamageFX") => reference(inp, RefKind::DamageFx, "None")?,
        Some("parseObjectCreationList") => reference(inp, RefKind::ObjectCreationList, "None")?,
        Some("parseUpgradeTemplate") => reference(inp, RefKind::Upgrade, "None")?,
        Some("parseSpecialPowerTemplate") => reference(inp, RefKind::SpecialPower, "None")?,
        Some("parseScience") => reference(inp, RefKind::Science, "None")?,
        Some("parseAudioEventRTS") | Some("parseDynamicAudioEventRTS") => reference(inp, RefKind::Audio, "NoSound")?,
        Some("parseMappedImage") => reference(inp, RefKind::MappedImage, "")?,
        Some("parseAndTranslateLabel") => reference(inp, RefKind::Label, "")?,
        _ => {
            // BitFlags<N> specialisations: `<Type>::parseFromINI` / `parseSingleBitFromINI`.
            if let Some(ty) = p.strip_suffix("::parseFromINI") {
                Value::Bits(parse_bitstring(inp, names(&format!("{ty}::s_bitNameList"))?)?)
            } else if let Some(ty) = p.strip_suffix("::parseSingleBitFromINI") {
                let (index, name) = scan_index(inp.next()?, names(&format!("{ty}::s_bitNameList"))?)?;
                Value::Index { name, index }
            } else if p == "parseAsciiStringLC" {
                Value::Str(next_ascii_string(inp).to_ascii_lowercase())
            } else {
                return qualified(p, def, inp);
            }
        }
    })
}

/// Class-qualified parsers outside `INI::`.
fn qualified(p: &'static str, def: &FieldDef, inp: &mut In) -> R<Value> {
    let real = |inp: &mut In| -> R<Value> { Ok(Value::Real(scan_real(inp.next()?)?)) };
    Ok(match p {
        // ParticleSys.cpp: "low high frame" and "R:r G:g B:b frame".
        "ParticleSystemTemplate::parseRandomKeyframe" => Value::Tuple(vec![
            Value::Real(scan_real(inp.next()?)?),
            Value::Real(scan_real(inp.next()?)?),
            Value::UInt(scan_uint(inp.next()?)?),
        ]),
        "ParticleSystemTemplate::parseRGBColorKeyframe" => {
            let mut c = [0f32; 3];
            for (i, l) in ["R", "G", "B"].iter().enumerate() {
                c[i] = ranged(scan_int(inp.sub(l)?)?, 0, 255, "a colour component")? as f32 / 255.0;
            }
            Value::Tuple(vec![Value::Rgb(c), Value::UInt(scan_uint(inp.next()?)?)])
        }
        // ControlBar.cpp: unknown buttons are a fatal error, checked when resolving refs.
        "CommandSet::parseCommandButton" => Value::Ref { kind: RefKind::CommandButton, name: Some(text(inp.next()?)) },
        "CommandButton::parseCommand" => {
            let (index, name) = scan_index(inp.next()?, names("TheGuiCommandNames")?)?;
            Value::Index { name, index }
        }
        // Armor.cpp: "<DamageType or Default> <percent>".
        "ArmorTemplate::parseArmorCoefficients" => {
            let dt = inp.next()?;
            let pct = scan_real(inp.next()?)? / 100.0;
            let target = if dt.eq_ignore_ascii_case(b"Default") {
                Value::Str("Default".into())
            } else {
                let (index, name) = scan_index(dt, names("DamageTypeFlags::s_bitNameList")?)?;
                Value::Index { name, index }
            };
            Value::Tuple(vec![target, Value::Real(pct)])
        }
        // Geometry.cpp.
        "GeometryInfo::parseGeometryType" => {
            let (index, name) = scan_index(inp.next()?, names("GeometryNames")?)?;
            Value::Index { name, index }
        }
        "GeometryInfo::parseGeometryIsSmall" => Value::Bool(scan_bool(inp.next()?)?),
        "GeometryInfo::parseGeometryMajorRadius"
        | "GeometryInfo::parseGeometryMinorRadius"
        | "GeometryInfo::parseGeometryHeight" => real(inp)?,
        // Image.cpp: pixel rectangle; UVs are derived once the texture size is known.
        "Image::parseImageCoords" => Value::Tuple(
            ["Left", "Top", "Right", "Bottom"]
                .iter()
                .map(|l| Ok(Value::Int(scan_int(inp.sub(l)?)?)))
                .collect::<R<Vec<_>>>()?,
        ),
        "Image::parseImageStatus" => Value::Bits(parse_bitstring(inp, names("imageStatusNames")?)?),
        // AIUpdate.cpp: "<set> <locomotor>..."; None entries are skipped.
        "AIUpdateModuleData::parseLocomotorSet" => {
            let (index, name) = scan_index(inp.next()?, names("TheLocomotorSetNames")?)?;
            let mut locos = Vec::new();
            let mut tok = Some(inp.next()?);
            while let Some(t) = tok {
                if !t.is_empty() && !t.eq_ignore_ascii_case(b"None") {
                    locos.push(Value::Ref { kind: RefKind::Locomotor, name: Some(text(t)) });
                }
                tok = inp.next_or_none();
            }
            Value::Tuple(vec![Value::Index { name, index }, Value::Tuple(locos)])
        }
        // ThingTemplate.cpp: a fixed number of ints, count in user data.
        "ThingTemplate::parseIntList" => {
            let n = scan_int(def.user.trim().as_bytes()).unwrap_or(0).max(0);
            Value::Tuple((0..n).map(|_| Ok(Value::Int(scan_int(inp.next()?)?))).collect::<R<Vec<_>>>()?)
        }
        // Weapon.cpp: "N" or "Min:N Max:M" milliseconds, stored as whole frames.
        "WeaponTemplate::parseShotDelay" => {
            let frames = |ms: i32| (ms as f32 * LOGICFRAMES_PER_MSEC).ceil() as i32;
            let tok = inp.next_or_none_with(SEPS_COLON).ok_or_else(|| ValueError::Invalid("missing value".into()))?;
            let (min, max) = if tok.eq_ignore_ascii_case(b"Min") {
                let min = scan_int(inp.next_with(SEPS_COLON)?)?;
                match inp.next_or_none_with(SEPS_COLON) {
                    Some(t) if t.eq_ignore_ascii_case(b"Max") => (min, scan_int(inp.next_with(SEPS_COLON)?)?),
                    _ => (min, min),
                }
            } else {
                let v = scan_int(tok)?;
                (v, v)
            };
            Value::Tuple(vec![Value::Int(frames(min)), Value::Int(frames(max))])
        }
        // WeaponSet.cpp: "<slot> <weapon>".
        "WeaponTemplateSet::parseWeapon" => {
            let (index, name) = scan_index(inp.next()?, names("TheWeaponSlotTypeNames")?)?;
            Value::Tuple(vec![Value::Index { name, index }, reference(inp, RefKind::Weapon, "None")?])
        }
        // W3DModelDraw.cpp: AliasConditionState reads a ModelConditionFlags set.
        "W3DModelDrawModuleData::parseConditionState" if def.user.contains("PARSE_ALIAS") => {
            Value::Bits(parse_bitstring(inp, names("ModelConditionFlags::s_bitNameList")?)?)
        }
        // TransitionDamageFX.cpp: "<loc info> PSys:<name>" / "<loc info> FXList:<name>".
        "TransitionDamageFXModuleData::parseParticleSystem" => {
            let loc = fx_loc_info(inp)?;
            expect_label(inp, "psys")?;
            Value::Tuple(vec![loc, reference(inp, RefKind::ParticleSystem, "None")?])
        }
        // TransitionDamageFX.cpp. Quirk: the original passes `store` and `&info->ocl`
        // to parseObjectCreationList in the wrong order, so the OCL pointer overwrites
        // the start of the location info and `info->ocl` stays null. These transition
        // OCLs never fire in the original; the simulation must reproduce that.
        "TransitionDamageFXModuleData::parseObjectCreationList" => {
            let loc = fx_loc_info(inp)?;
            expect_label(inp, "ocl")?;
            Value::Tuple(vec![loc, reference(inp, RefKind::ObjectCreationList, "None")?])
        }
        "TransitionDamageFXModuleData::parseFXList" => {
            let loc = fx_loc_info(inp)?;
            expect_label(inp, "fxlist")?;
            Value::Tuple(vec![loc, reference(inp, RefKind::FxList, "None")?])
        }
        // WeaponSet.cpp: "<slot> <command sources>".
        "WeaponTemplateSet::parseAutoChoose" => {
            let (index, name) = scan_index(inp.next()?, names("TheWeaponSlotTypeNames")?)?;
            let bits = parse_bitstring(inp, names("TheCommandSourceMaskNames")?)?;
            Value::Tuple(vec![Value::Index { name, index }, Value::Bits(bits)])
        }
        // Credits.cpp.
        "CreditsManager::parseText" => Value::Str(next_quoted_string(inp)?),
        "CreditsManager::parseBlank" => Value::Tuple(vec![]),
        // Weapon.cpp: "<condition> <field> <percent>".
        "WeaponTemplate::parseWeaponBonusSet" | "WeaponBonusSet::parseWeaponBonusSetPtr" => {
            let (bi, bn) = scan_index(inp.next()?, names("TheWeaponBonusNames")?)?;
            let (fi, fname) = scan_index(inp.next()?, names("TheWeaponBonusFieldNames")?)?;
            let pct = scan_real(inp.next()?)? / 100.0;
            Value::Tuple(vec![
                Value::Index { name: bn, index: bi },
                Value::Index { name: fname, index: fi },
                Value::Real(pct),
            ])
        }
        "WeaponTemplate::parseScatterTarget" => Value::Coord2([scan_real(inp.sub("X")?)?, scan_real(inp.sub("Y")?)?]),
        // ThingTemplate.cpp.
        "ThingTemplate::parseMaxSimultaneous" => {
            let t = inp.next()?;
            if t.eq_ignore_ascii_case(b"DeterminedBySuperweaponRestriction") {
                Value::Str("DeterminedBySuperweaponRestriction".into())
            } else {
                Value::Int(ranged(scan_int(t)?, 0, 65535, "MaxSimultaneousOfType")?)
            }
        }
        // The module tag must exist on the template; checked when templates are built.
        "ThingTemplate::parseRemoveModule" => Value::Str(text(inp.next()?)),
        "WeaponTemplateSet::parsePreferredAgainst" => {
            let (index, name) = scan_index(inp.next()?, names("TheWeaponSlotTypeNames")?)?;
            let kinds = parse_bitstring(inp, names("KindOfMaskType::s_bitNameList")?)?;
            Value::Tuple(vec![Value::Index { name, index }, Value::Bits(kinds)])
        }
        // Anim2D.cpp.
        "Anim2DTemplate::parseImage" => reference(inp, RefKind::MappedImage, "")?,
        "Anim2DTemplate::parseNumImages" => {
            let n = scan_uint(inp.next()?)?;
            if n < 1 {
                return invalid("NumberImages must be at least 1");
            }
            Value::UInt(n)
        }
        // Anim2D.cpp: images "<base>000", "<base>001", ... up to NumberImages, which
        // must come first in the block; resolved when the animation is built.
        "Anim2DTemplate::parseImageSequence" => Value::Str(next_ascii_string(inp)),
        // PlayerTemplate.cpp: "<object> <percent>" and "<object> <veterancy>".
        "PlayerTemplate::parseProductionCostChange" | "PlayerTemplate::parseProductionTimeChange" => {
            let object = Value::Ref { kind: RefKind::Object, name: Some(text(inp.next()?)) };
            Value::Tuple(vec![object, Value::Real(scan_real(inp.next()?)? / 100.0)])
        }
        "PlayerTemplate::parseProductionVeterancyLevel" => {
            let object = Value::Ref { kind: RefKind::Object, name: Some(text(inp.next()?)) };
            let (index, name) = scan_index(inp.next()?, names("TheVeterancyNames")?)?;
            Value::Tuple(vec![object, Value::Index { name, index }])
        }
        "NameKeyGenerator::parseStringAsNameKeyType" => Value::Str(text(inp.next()?)),
        // Eva.cpp: any EVA message name except the trailing EVA_INVALID.
        "Eva::parseEvaMessageFromIni" => {
            let list = names("TheEvaMessageNames")?;
            let list = &list[..list.len().saturating_sub(1)];
            let t = inp.next()?;
            match list.iter().position(|n| n.as_bytes().eq_ignore_ascii_case(t)) {
                Some(i) => Value::Index { name: list[i], index: i as i32 },
                None => return invalid(format!("'{}' is not an EVA message", text(t))),
            }
        }
        // TurretAI.cpp: "<slot> <degrees>" and "<slot> <factor>".
        "TurretAIData::parseTurretSweep" | "TurretAIData::parseTurretSweepSpeed" => {
            let (index, name) = scan_index(inp.next()?, names("TheWeaponSlotTypeNames")?)?;
            let v = scan_real(inp.next()?)?;
            let v = if p.ends_with("Speed") { v } else { v * RADS_PER_DEGREE };
            Value::Tuple(vec![Value::Index { name, index }, Value::Real(v)])
        }
        // GlobalLanguage.cpp: "\"Font Name\" <size> <bold>".
        "GlobalLanguage::parseFontDesc" => Value::Tuple(vec![
            Value::Str(next_quoted_string(inp)?),
            Value::Int(scan_int(inp.next()?)?),
            Value::Bool(scan_bool(inp.next()?)?),
        ]),
        "GlobalLanguage::parseFontFileName" => Value::Str(next_ascii_string(inp)),
        "PlayerTemplate::parseStartMoney" => Value::Int(scan_int(inp.next()?)?),
        "Money::parseMoneyAmount" => Value::UInt(scan_uint(inp.next()?)?),
        // CrateSystem.cpp: "<crate> <chance>".
        "CrateTemplate::parseCrateCreationEntry" => Value::Tuple(vec![
            Value::Ref { kind: RefKind::Crate, name: Some(text(inp.next()?)) },
            Value::Real(scan_real(inp.next()?)?),
        ]),
        "BoneFXUpdateModuleData::parseFXList" => bone_fx(inp, "fxlist", RefKind::FxList)?,
        "BoneFXUpdateModuleData::parseObjectCreationList" => bone_fx(inp, "ocl", RefKind::ObjectCreationList)?,
        "BoneFXUpdateModuleData::parseParticleSystem" => bone_fx(inp, "psys", RefKind::ParticleSystem)?,
        "CreateCrateDieModuleData::parseCrateData" => {
            Value::Ref { kind: RefKind::Crate, name: Some(text(inp.next()?)) }
        }
        // AI.cpp: a science for an AI skill set.
        "AI::parseScience" => reference(inp, RefKind::Science, "None")?,
        _ => return Err(ValueError::Unimplemented(p)),
    })
}

/// `AsciiString::isNone`: case-insensitive "None".
fn is_none(s: &str) -> bool {
    s.eq_ignore_ascii_case("none")
}

/// Parses a field, dispatching file-local `static` parsers by their source file.
///
/// Several files define statics with the same name and different behaviour (e.g.
/// `parseOCL` in five death/collapse modules), so unqualified parsers are matched on
/// `(file, name)`.
pub fn parse_field(table: &crate::schema::TableDef, def: &FieldDef, raw: &str) -> R<Value> {
    if def.parser.contains("::") {
        return parse_value(def, raw);
    }
    let file = table.file.rsplit('/').next().unwrap_or(table.file);
    let bytes: Vec<u8> = raw.chars().map(|c| c as u32 as u8).collect();
    let mut inp = In { t: Tokens::new(&bytes) };
    let inp = &mut inp;
    let lc = |s: String| s.to_ascii_lowercase();
    Ok(match (file, def.parser) {
        // W3DModelDraw.cpp condition-state fields.
        ("W3DModelDraw.cpp", "parseParticleSysBone") => {
            let bone = lc(next_ascii_string(inp));
            Value::Tuple(vec![Value::Str(bone), reference(inp, RefKind::ParticleSystem, "None")?])
        }
        ("W3DModelDraw.cpp", "parseAnimation") => {
            let anim = lc(next_ascii_string(inp));
            let distance = match inp.next_or_none() {
                Some(t) => scan_real(t)?,
                None => 0.0,
            };
            let repeat = match inp.next_or_none() {
                Some(t) => scan_int(t)?.max(1),
                None => 1,
            };
            Value::Tuple(vec![Value::Str(anim), Value::Real(distance), Value::Int(repeat)])
        }
        ("W3DModelDraw.cpp", "parseLowercaseNameKey") => Value::Str(lc(text(inp.next()?))),
        ("W3DModelDraw.cpp", "parseBoneNameKey") => {
            let b = lc(text(inp.next()?));
            Value::Str(if is_none(&b) { String::new() } else { b })
        }
        ("W3DModelDraw.cpp", "parseWeaponBoneName") => {
            let (index, name) = scan_index(inp.next()?, names("TheWeaponSlotTypeNames")?)?;
            let b = lc(next_ascii_string(inp));
            Value::Tuple(vec![Value::Index { name, index }, Value::Str(if is_none(&b) { String::new() } else { b })])
        }
        ("W3DModelDraw.cpp", "parseShowHideSubObject") => {
            // user data non-NULL means "hide"; "None" clears the list.
            let hide = !matches!(def.user.trim(), "NULL" | "0" | "");
            let mut subs = Vec::new();
            let mut s = lc(next_ascii_string(inp));
            if !is_none(&s) {
                while !s.is_empty() {
                    subs.push(s);
                    s = lc(next_ascii_string(inp));
                }
            }
            Value::Tuple(vec![Value::Bool(hide), Value::Strings(subs)])
        }
        ("W3DModelDraw.cpp", "parseRealRange") => {
            Value::Tuple(vec![Value::Real(scan_real(inp.next()?)?), Value::Real(scan_real(inp.next()?)?)])
        }
        // INIMapCache.cpp.
        ("INIMapCache.cpp", "parseSupplyPositionCoord3D" | "parseTechPositionsCoord3D") => {
            Value::Coord3([scan_real(inp.sub("X")?)?, scan_real(inp.sub("Y")?)?, scan_real(inp.sub("Z")?)?])
        }
        // ThingTemplate.cpp.
        ("ThingTemplate.cpp", "parseArbitrarySoundsIntoMap") => reference(inp, RefKind::Audio, "NoSound")?,
        ("ThingTemplate.cpp", "parsePrerequisiteUnit") => {
            // All names on one line are alternatives (OR).
            let mut v = vec![text(inp.next()?)];
            v.extend(std::iter::from_fn(|| inp.next_or_none()).map(text));
            Value::Strings(v)
        }
        // ObjectCreationList.cpp: the loop reads a token in its body *and* in its
        // step, so every second name is skipped. Kept as the original behaves.
        ("ObjectCreationList.cpp", "parseDebrisObjectNames") => {
            let mut v = Vec::new();
            let mut tok = Some(inp.next()?);
            while let Some(t) = tok {
                v.push(text(t));
                let _skipped = inp.next_or_none();
                tok = inp.next_or_none();
            }
            Value::Strings(v)
        }
        // Death and collapse behaviours: "[<phase>] <name>..." (first name required).
        ("SlowDeathBehavior.cpp", p @ ("parseFX" | "parseOCL" | "parseWeapon")) => {
            phase_refs(inp, "TheSlowDeathPhaseNames", list_kind(p))?
        }
        ("StructureCollapseUpdate.cpp", p @ ("parseFX" | "parseOCL")) => {
            phase_refs(inp, "TheStructureCollapsePhaseNames", list_kind(p))?
        }
        ("StructureToppleUpdate.cpp", p @ ("parseFX" | "parseOCL")) => {
            phase_refs(inp, "TheStructureTopplePhaseNames", list_kind(p))?
        }
        ("InstantDeathBehavior.cpp", p @ ("parseFX" | "parseOCL" | "parseWeapon")) => ref_list(inp, list_kind(p))?,
        // BridgeBehavior.cpp: "FX:<name> Delay:<ms> [Bone:<bone>]" (OCL likewise).
        ("BridgeBehavior.cpp", p @ ("parseFX" | "parseOCL")) => {
            let (label, kind) =
                if p == "parseFX" { ("FX", RefKind::FxList) } else { ("OCL", RefKind::ObjectCreationList) };
            expect_label(inp, label)?;
            let r = reference(inp, kind, "None")?;
            expect_label(inp, "Delay")?;
            let delay = (scan_uint(inp.next()?)? as f32 * LOGICFRAMES_PER_MSEC).ceil() as u32;
            let bone = match inp.next_or_none_with(SEPS_COLON) {
                None => String::new(),
                Some(t) if t.eq_ignore_ascii_case(b"Bone") => next_ascii_string(inp),
                Some(t) => return invalid(format!("expected 'Bone', found '{}'", text(t))),
            };
            Value::Tuple(vec![r, Value::UInt(delay), Value::Str(bone)])
        }
        // Weapon.cpp: one value copied to every veterancy level.
        ("Weapon.cpp", "parseAllVetLevelsFXList") => reference(inp, RefKind::FxList, "None")?,
        ("Weapon.cpp", "parseAllVetLevelsAsciiString") => Value::Str(next_ascii_string(inp)),
        // OCLSpecialPower.cpp: "<science> <ocl>".
        ("OCLSpecialPower.cpp", "parseOCLUpgradePair") => Value::Tuple(vec![
            reference(inp, RefKind::Science, "None")?,
            reference(inp, RefKind::ObjectCreationList, "None")?,
        ]),
        // INIAudioEventInfo.cpp: percentages become pitch multipliers.
        ("INIAudioEventInfo.cpp", "parsePitchShift") => {
            let min = scan_real(inp.next()?)?;
            let max = scan_real(inp.next()?)?;
            Value::Tuple(vec![Value::Real(1.0 + min / 100.0), Value::Real(1.0 + max / 100.0)])
        }
        ("INIAudioEventInfo.cpp", "parseDelay") => {
            Value::Tuple(vec![Value::Int(scan_int(inp.next()?)?), Value::Int(scan_int(inp.next()?)?)])
        }
        // TurretAI.cpp: weapon slots controlled by the turret.
        ("TurretAI.cpp", "parseTWS") => {
            let slots = names("TheWeaponSlotTypeNames")?;
            let mut v = Vec::new();
            let mut tok = Some(inp.next()?);
            while let Some(t) = tok {
                let (index, name) = scan_index(t, slots)?;
                v.push(Value::Index { name, index });
                tok = inp.next_or_none();
            }
            Value::Tuple(v)
        }
        // DamageFX.cpp: "[<veterancy>] <DamageType|Default> <fxlist>".
        ("DamageFX.cpp", "parseMajorFXList" | "parseMinorFXList") => {
            let (vet, damage) = damage_fx_prefix(inp, def)?;
            Value::Tuple(vec![vet, damage, reference(inp, RefKind::FxList, "None")?])
        }
        // Friction is given per second and stored per frame (three identical copies).
        ("Locomotor.cpp" | "ObjectCreationList.cpp" | "PhysicsUpdate.cpp", "parseFrictionPerSec") => {
            Value::Real(scan_real(inp.next()?)? * SECONDS_PER_LOGICFRAME)
        }
        // PhysicsUpdate.cpp: a fall height, converted to a speed with GameData's gravity
        // (sqrt(|2 g h|)) when templates are built.
        ("PhysicsUpdate.cpp", "parseHeightToSpeed") => Value::Real(scan_real(inp.next()?)?),
        // RiderChangeContain.cpp: "<object> <model condition> <weapon set flag> <status>
        // <command set> <locomotor set>".
        ("RiderChangeContain.cpp", "parseRiderInfo") => {
            let object = Value::Ref { kind: RefKind::Object, name: Some(text(inp.next()?)) };
            let mut idx = |list: &str| -> R<Value> {
                let (index, name) = scan_index(inp.next()?, names(list)?)?;
                Ok(Value::Index { name, index })
            };
            let cond = idx("ModelConditionFlags::s_bitNameList")?;
            let wset = idx("WeaponSetFlags::s_bitNameList")?;
            let status = idx("ObjectStatusMaskType::s_bitNameList")?;
            let command_set = Value::Str(text(inp.next()?));
            let (index, name) = scan_index(inp.next()?, names("TheLocomotorSetNames")?)?;
            Value::Tuple(vec![object, cond, wset, status, command_set, Value::Index { name, index }])
        }
        // Weapon.cpp veterancy variants.
        ("Weapon.cpp", "parseAllVetLevelsPSys") => reference(inp, RefKind::ParticleSystem, "None")?,
        ("Weapon.cpp", p @ ("parsePerVetLevelPSys" | "parsePerVetLevelFXList")) => {
            let (index, name) = scan_index(inp.next()?, names("TheVeterancyNames")?)?;
            let kind = if p == "parsePerVetLevelPSys" { RefKind::ParticleSystem } else { RefKind::FxList };
            Value::Tuple(vec![Value::Index { name, index }, reference(inp, kind, "None")?])
        }
        // DamageFX.cpp: "[<veterancy>] <DamageType|Default> <amount or ms>".
        ("DamageFX.cpp", p @ ("parseAmount" | "parseTime")) => {
            let (vet, damage) = damage_fx_prefix(inp, def)?;
            let v = if p == "parseAmount" {
                Value::Real(scan_real(inp.next()?)?)
            } else {
                Value::UInt((scan_uint(inp.next()?)? as f32 * LOGICFRAMES_PER_MSEC).ceil() as u32)
            };
            Value::Tuple(vec![vet, damage, v])
        }
        // OCLUpdate.cpp: "Faction:<side> OCL:<ocl>".
        ("OCLUpdate.cpp", "parseFactionObjectCreationList") => {
            expect_label(inp, "Faction")?;
            let faction =
                text(inp.next_or_none_with(SEPS_COLON).ok_or_else(|| ValueError::Invalid("missing faction".into()))?);
            expect_label(inp, "OCL")?;
            Value::Tuple(vec![Value::Str(faction), reference(inp, RefKind::ObjectCreationList, "None")?])
        }
        // ObjectCreationList.cpp: initial, flying and final animation names.
        ("ObjectCreationList.cpp", "parseAnimSet") => {
            Value::Strings(vec![next_ascii_string(inp), next_ascii_string(inp), next_ascii_string(inp)])
        }
        // CashBountyPower.cpp: "<science> <percent>".
        ("CashBountyPower.cpp", "parseBountyUpgradePair") => {
            let science = reference(inp, RefKind::Science, "None")?;
            Value::Tuple(vec![science, Value::Real(scan_real(inp.next_with(SEPS_PERCENT)?)? / 100.0)])
        }
        ("Weapon.cpp", "parsePerVetLevelAsciiString") => {
            let (index, name) = scan_index(inp.next()?, names("TheVeterancyNames")?)?;
            Value::Tuple(vec![Value::Index { name, index }, Value::Str(next_ascii_string(inp))])
        }
        ("CashHackSpecialPower.cpp", "parseCashHackUpgradePair") => {
            Value::Tuple(vec![reference(inp, RefKind::Science, "None")?, Value::Int(scan_int(inp.next()?)?)])
        }
        // "<object> [count]" in several contain modules and ProductionUpdate.
        ("TransportContain.cpp" | "MobNexusContain.cpp", "parseInitialPayload")
        | ("GarrisonContain.h", "parseInitialRoster")
        | ("ProductionUpdate.cpp", "parseAppendQuantityModifier") => {
            let name = text(inp.next()?);
            let count = match inp.next_or_none() {
                Some(t) => scan_int(t)?,
                None => 1,
            };
            Value::Tuple(vec![Value::Ref { kind: RefKind::Object, name: Some(name) }, Value::Int(count)])
        }
        // TerrainRoads.cpp: "Transition:<Damage|Repair> ToState:<state> EffectNum:<1..3> OCL|FX:<name>".
        ("TerrainRoads.cpp", p @ ("parseTransitionToOCL" | "parseTransitionToFX")) => {
            let t = inp.sub("Transition")?;
            let damage = if t.eq_ignore_ascii_case(b"Damage") {
                true
            } else if t.eq_ignore_ascii_case(b"Repair") {
                false
            } else {
                return invalid(format!("Transition must be Damage or Repair, found '{}'", text(t)));
            };
            let (index, name) = scan_index(inp.sub("ToState")?, names("TheBodyDamageTypeNames")?)?;
            // MAX_BRIDGE_BODY_FX = 3 (TerrainRoads.h).
            let effect = scan_int(inp.sub("EffectNum")?)? - 1;
            ranged(effect, 0, 2, "EffectNum - 1")?;
            let (label, kind) = if p == "parseTransitionToOCL" {
                ("OCL", RefKind::ObjectCreationList)
            } else {
                ("FX", RefKind::FxList)
            };
            let target = Value::Ref { kind, name: Some(text(inp.sub(label)?)) };
            Value::Tuple(vec![Value::Bool(damage), Value::Index { name, index }, Value::Int(effect), target])
        }
        // StructureToppleUpdate.cpp: "<degrees> <fxlist>".
        ("StructureToppleUpdate.cpp", "parseAngleFX") => {
            let angle = scan_real(inp.next()?)? * PI / 180.0;
            Value::Tuple(vec![Value::Real(angle), reference(inp, RefKind::FxList, "None")?])
        }
        ("ThingTemplate.cpp", "parseArbitraryFXIntoMap") => reference(inp, RefKind::FxList, "None")?,
        // FlightDeckBehavior.cpp: optional start and end bone names.
        ("FlightDeckBehavior.cpp", "parseRunwayStrip") => {
            Value::Strings(std::iter::from_fn(|| inp.next_or_none()).take(2).map(text).collect())
        }
        // AutoDepositUpdate.cpp (also used by MoneyCrateCollide): "UpgradeType:<u> Boost:<n>".
        ("AutoDepositUpdate.h" | "MoneyCrateCollide.h", "parseUpgradePair") => {
            expect_label(inp, "UpgradeType")?;
            let up =
                text(inp.next_or_none_with(SEPS_COLON).ok_or_else(|| ValueError::Invalid("missing upgrade".into()))?);
            expect_label(inp, "Boost")?;
            let boost = scan_int(inp.next_with(SEPS_COLON)?)?;
            Value::Tuple(vec![Value::Ref { kind: RefKind::Upgrade, name: Some(up) }, Value::Int(boost)])
        }
        // GameAudio.cpp: speaker setup name, translated by the audio device.
        ("GameAudio.cpp", "parseSpeakerType") => Value::Str(next_ascii_string(inp)),
        // INIDrawGroupInfo.cpp: pixel (int) or percent offset; user data picks X or Y.
        ("INIDrawGroupInfo.cpp", "parseInt") => Value::Int(scan_int(inp.next()?)?),
        ("INIDrawGroupInfo.cpp", "parsePercentToReal") => Value::Real(scan_real(inp.next_with(SEPS_PERCENT)?)? / 100.0),
        ("ThingTemplate.cpp", "parsePrerequisiteScience") => {
            Value::Ref { kind: RefKind::Science, name: Some(text(inp.next()?)) }
        }
        // FXList.cpp: function-local lookup lists.
        ("FXList.cpp", p @ ("parseShakeType" | "parseScorchType")) => {
            let id = if p == "parseShakeType" { "shakeTypeNames" } else { "scorchTypeNames" };
            let list = schema()
                .lookup_list(id)
                .ok_or_else(|| ValueError::Invalid(format!("lookup list '{id}' not extracted")))?;
            let t = inp.next()?;
            match list.iter().position(|(n, _)| n.as_bytes().eq_ignore_ascii_case(t)) {
                Some(i) => Value::Index { name: list[i].0, index: i as i32 },
                None => return invalid(format!("'{}' is not a valid name", text(t))),
            }
        }
        // ObjectCreationList.cpp: "<object> [count]".
        ("ObjectCreationList.cpp", "parsePayload") => {
            let name = text(inp.next()?);
            let count = match inp.next_or_none() {
                Some(t) => scan_int(t)?,
                None => 1,
            };
            Value::Tuple(vec![Value::Ref { kind: RefKind::Object, name: Some(name) }, Value::Int(count)])
        }
        _ => return parse_value(def, raw),
    })
}

/// `parseCommonStuff` (DamageFX.cpp): an optional veterancy level (when the field's
/// user data is the veterancy name list) then a damage type or `Default`.
fn damage_fx_prefix(inp: &mut In, def: &FieldDef) -> R<(Value, Value)> {
    let vet = if def.user.contains("TheVeterancyNames") {
        let (index, name) = scan_index(inp.next()?, names("TheVeterancyNames")?)?;
        Value::Index { name, index }
    } else {
        Value::Str("All".into())
    };
    let dt = inp.next()?;
    let damage = if dt.eq_ignore_ascii_case(b"Default") {
        Value::Str("Default".into())
    } else {
        let (index, name) = scan_index(dt, names("DamageTypeFlags::s_bitNameList")?)?;
        Value::Index { name, index }
    };
    Ok((vet, damage))
}

/// BoneFXUpdate.cpp: "Bone:<b> OnlyOnce:<bool> <min ms> <max ms> <Label>:<name>".
fn bone_fx(inp: &mut In, label: &str, kind: RefKind) -> R<Value> {
    expect_label(inp, "bone")?;
    let bone = text(inp.next()?);
    expect_label(inp, "onlyonce")?;
    let once = scan_bool(inp.next()?)?;
    let min = scan_real(inp.next()?)? * LOGICFRAMES_PER_MSEC;
    let max = scan_real(inp.next()?)? * LOGICFRAMES_PER_MSEC;
    expect_label(inp, label)?;
    Ok(Value::Tuple(vec![
        Value::Str(bone),
        Value::Bool(once),
        Value::Real(min),
        Value::Real(max),
        reference(inp, kind, "None")?,
    ]))
}

fn list_kind(parser: &str) -> RefKind {
    match parser {
        "parseFX" => RefKind::FxList,
        "parseOCL" => RefKind::ObjectCreationList,
        _ => RefKind::Weapon,
    }
}

/// One or more references; unknown names and `None` become null in the original.
fn ref_list(inp: &mut In, kind: RefKind) -> R<Value> {
    let mut v = vec![inp.next()?];
    v.extend(std::iter::from_fn(|| inp.next_or_none()));
    Ok(Value::Tuple(
        v.into_iter().map(|t| Value::Ref { kind, name: (!t.eq_ignore_ascii_case(b"None")).then(|| text(t)) }).collect(),
    ))
}

fn phase_refs(inp: &mut In, phases: &str, kind: RefKind) -> R<Value> {
    let (index, name) = scan_index(inp.next()?, names(phases)?)?;
    Ok(Value::Tuple(vec![Value::Index { name, index }, ref_list(inp, kind)?]))
}

/// A `Label:` read with the colon separators, compared case-insensitively.
fn expect_label(inp: &mut In, label: &str) -> R<()> {
    let t = inp.next_with(SEPS_COLON)?;
    if t.eq_ignore_ascii_case(label.as_bytes()) {
        Ok(())
    } else {
        invalid(format!("expected '{label}', found '{}'", text(t)))
    }
}

/// `parseFXLocInfo` (TransitionDamageFX.cpp): "Bone:<b> RandomBone:<bool>" or "Loc:X:.. Y:.. Z:..".
fn fx_loc_info(inp: &mut In) -> R<Value> {
    let t = inp.next_with(SEPS_COLON)?;
    if t.eq_ignore_ascii_case(b"bone") {
        let bone = text(inp.next()?);
        expect_label(inp, "randombone")?;
        Ok(Value::Tuple(vec![Value::Str(bone), Value::Bool(scan_bool(inp.next()?)?)]))
    } else if t.eq_ignore_ascii_case(b"loc") {
        Ok(Value::Coord3([scan_real(inp.sub("X")?)?, scan_real(inp.sub("Y")?)?, scan_real(inp.sub("Z")?)?]))
    } else {
        invalid(format!("expected Bone or Loc, found '{}'", text(t)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn def(parser: &'static str, user: &'static str) -> FieldDef {
        FieldDef { name: Some("X"), parser, user }
    }

    fn v(parser: &'static str, user: &'static str, raw: &str) -> R<Value> {
        parse_value(&def(parser, user), raw)
    }

    #[test]
    fn numbers_follow_sscanf() {
        assert_eq!(scan_int(b"10abc"), Ok(10));
        assert_eq!(scan_int(b"-7"), Ok(-7));
        assert!(scan_int(b"abc").is_err());
        assert_eq!(scan_uint(b"-1"), Ok(u32::MAX));
        assert_eq!(scan_real(b"1.5f"), Ok(1.5));
        assert_eq!(scan_real(b".5"), Ok(0.5));
        assert_eq!(scan_real(b"3."), Ok(3.0));
        assert_eq!(scan_real(b"2e2x"), Ok(200.0));
        assert_eq!(scan_real(b"2e"), Ok(2.0));
    }

    #[test]
    fn unit_conversions() {
        // 1000 ms at 30 fps = 30 frames; 50 ms = 1.5 -> ceil 2 frames.
        assert_eq!(v("INI::parseDurationUnsignedInt", "", "= 1000"), Ok(Value::UInt(30)));
        assert_eq!(v("INI::parseDurationUnsignedInt", "", "= 50"), Ok(Value::UInt(2)));
        assert_eq!(v("INI::parseVelocityReal", "", "= 30"), Ok(Value::Real(30.0 * (1.0 / 30.0))));
        assert_eq!(v("INI::parsePercentToReal", "", "= 25%"), Ok(Value::Real(0.25)));
        let Ok(Value::Real(a)) = v("INI::parseAngleReal", "", "= 180") else { panic!() };
        assert!((a - std::f32::consts::PI).abs() < 1e-6);
    }

    #[test]
    fn structured_values() {
        assert_eq!(v("INI::parseCoord3D", "", "= X:1 Y:-2.5 Z:3"), Ok(Value::Coord3([1.0, -2.5, 3.0])));
        assert_eq!(v("INI::parseColorInt", "", "= R:10 G:20 B:30"), Ok(Value::Rgba([10, 20, 30, 255])));
        assert_eq!(v("INI::parseBool", "", "= YES"), Ok(Value::Bool(true)));
        assert_eq!(v("INI::parseWeaponTemplate", "", "= None"), Ok(Value::Ref { kind: RefKind::Weapon, name: None }));
        assert_eq!(v("INI::parseAsciiString", "", "= \"Hello there\""), Ok(Value::Str("Hello there".into())));
    }

    #[test]
    fn flags() {
        let Ok(Value::Bits(b)) = v("KindOfMaskType::parseFromINI", "", "= SELECTABLE CAN_ATTACK") else { panic!() };
        assert!(b.replace);
        assert_eq!(b.set, [1, 3]);
        let Ok(Value::Bits(b)) = v("KindOfMaskType::parseFromINI", "", "= +SELECTABLE -OBSTACLE") else { panic!() };
        assert!(!b.replace);
        assert_eq!((b.set.as_slice(), b.clear.as_slice()), (&[1u32][..], &[0u32][..]));
        assert!(v("KindOfMaskType::parseFromINI", "", "= SELECTABLE +OBSTACLE").is_err());
        assert!(v("KindOfMaskType::parseFromINI", "", "= NOT_A_KINDOF").is_err());
        let Ok(Value::Bits(b)) = v("INI::parseDeathTypeFlags", "", "= NONE +NORMAL") else { panic!() };
        assert_eq!(b.set, [0]);
    }
}

#[cfg(test)]
mod schema_coverage {
    use super::*;
    use crate::schema::{Shape, TABLES, schema};

    /// Every one-line field in the extracted schema must have a ported parser, so that
    /// any mod's data can be read, not just the data sets tested so far.
    #[test]
    fn every_schema_parser_is_ported() {
        let mut missing = std::collections::BTreeSet::new();
        for t in TABLES {
            for f in t.fields {
                if !matches!(schema().shape(f), Shape::Field) {
                    continue;
                }
                if let Err(ValueError::Unimplemented(p)) = parse_field(t, f, "") {
                    let file = t.file.rsplit('/').next().unwrap_or(t.file);
                    missing.insert(if p.contains("::") { p.to_string() } else { format!("{file}:{p}") });
                }
            }
        }
        assert!(
            missing.is_empty(),
            "{} parsers not ported:\n{}",
            missing.len(),
            missing.into_iter().collect::<Vec<_>>().join("\n")
        );
    }
}

#[cfg(test)]
mod static_parsers {
    use super::*;
    use crate::schema::TableDef;

    fn field(file: &'static str, parser: &'static str, raw: &str) -> R<Value> {
        let def = FieldDef { name: Some("X"), parser, user: "NULL" };
        let table = TableDef { id: "test", file, fields: &[] };
        parse_field(&table, &def, raw)
    }

    #[test]
    fn debris_names_skip_every_second_token() {
        let v =
            field("GameEngine/Source/GameLogic/Object/ObjectCreationList.cpp", "parseDebrisObjectNames", "= A B C D E");
        assert_eq!(v, Ok(Value::Strings(vec!["A".into(), "C".into(), "E".into()])));
    }

    #[test]
    fn same_name_different_file() {
        // parseFX takes a phase in SlowDeathBehavior but not in InstantDeathBehavior.
        let slow = field("x/SlowDeathBehavior.cpp", "parseFX", "= FINAL FX_Boom").unwrap();
        let instant = field("x/InstantDeathBehavior.cpp", "parseFX", "= FINAL FX_Boom").unwrap();
        let Value::Tuple(s) = slow else { panic!() };
        assert!(matches!(s[0], Value::Index { name: "FINAL", .. }));
        let Value::Tuple(i) = instant else { panic!() };
        assert_eq!(i.len(), 2, "FINAL is read as an FX list name, as the original does");
    }

    #[test]
    fn bridge_transition() {
        let v = field(
            "x/TerrainRoads.cpp",
            "parseTransitionToOCL",
            "= Transition:Damage ToState:REALLYDAMAGED EffectNum:1 OCL:OCL_BridgeDebris",
        )
        .unwrap();
        let Value::Tuple(t) = v else { panic!() };
        assert_eq!(t[0], Value::Bool(true));
        assert_eq!(t[2], Value::Int(0));
        assert!(
            field(
                "x/TerrainRoads.cpp",
                "parseTransitionToOCL",
                "= Transition:Damage ToState:REALLYDAMAGED EffectNum:4 OCL:X"
            )
            .is_err()
        );
    }

    #[test]
    fn friction_is_per_frame() {
        assert_eq!(field("x/PhysicsUpdate.cpp", "parseFrictionPerSec", "= 30"), Ok(Value::Real(30.0 * (1.0 / 30.0))));
    }
}
