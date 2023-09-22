#![allow(dead_code)]

use crate::assets::*;
use crate::consts::*;
use crate::entity::*;
use encoding::label::encoding_from_whatwg_label;
use encoding::{CodecError, DecoderTrap, EncoderTrap, Encoding, EncodingRef, StringWriter};
use std::borrow::Cow;
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use unic::char::property::EnumeratedCharProperty;
use unic::ucd::normal::decompose_canonical;
use unic::ucd::{GeneralCategory, Name};

use cache_macro::cache;
use lru_cache::LruCache;

// Utils module

// check if character category contains one of categories_partial or
// if character category is exactly one of categories_exact or
// character is from range which has name, contains one of ranges_partial
fn in_category(
    character: &char,
    categories_exact: &[&str],
    categories_partial: &[&str],
    ranges_partial: &[&str],
) -> bool {
    // unicode category part
    let category = GeneralCategory::of(*character).abbr_name().to_string();
    if categories_exact.contains(&&*category)
        || categories_partial.iter().any(|&cp| category.contains(cp))
    {
        return true;
    }
    // unicode range part
    if !ranges_partial.is_empty() {
        if let Some(range) = unicode_range(character) {
            return ranges_partial.iter().any(|&r| range.contains(r));
        }
    }
    false
}

// check if character description contains at least one of patterns
fn in_description(character: &char, patterns: &[&str]) -> bool {
    if let Some(description) = Name::of(*character) {
        let description = format!("{}", description);
        return patterns.iter().any(|&s| description.contains(s));
    }
    false
}

#[cache(LruCache: LruCache::new(*UTF8_MAXIMAL_ALLOCATION))]
pub(crate) fn is_punctuation(character: &char) -> bool {
    in_category(character, &[], &["P"], &["Punctuation"])
}

#[cache(LruCache: LruCache::new(*UTF8_MAXIMAL_ALLOCATION))]
pub(crate) fn is_symbol(character: &char) -> bool {
    in_category(character, &[], &["N", "S"], &["Forms"])
}

//#[cache(LruCache: LruCache::new(*UTF8_MAXIMAL_ALLOCATION))]
pub(crate) fn is_emoticon(character: &char) -> bool {
    in_category(character, &[], &[], &["Emoticons"])
}

#[cache(LruCache: LruCache::new(*UTF8_MAXIMAL_ALLOCATION))]
pub(crate) fn is_separator(character: &char) -> bool {
    if character.is_whitespace() || ['｜', '+', '<', '>'].contains(character) {
        return true;
    }
    in_category(character, &["Po", "Pd", "Pc"], &["Z"], &[])
}

//#[cache(LruCache: LruCache::new(*UTF8_MAXIMAL_ALLOCATION))]
pub(crate) fn is_unprintable(character: &char) -> bool {
    !character.is_whitespace()
        && !character.is_ascii_graphic()
        && !['\x1A', '\u{FEFF}'].contains(character)
        && in_category(character, &["Cc"], &[], &["Control character"])
}

#[cache(LruCache: LruCache::new(*UTF8_MAXIMAL_ALLOCATION))]
pub(crate) fn is_accentuated(character: &char) -> bool {
    let patterns = [
        "WITH GRAVE",
        "WITH ACUTE",
        "WITH CEDILLA",
        "WITH DIAERESIS",
        "WITH CIRCUMFLEX",
        "WITH TILDE",
    ];
    in_description(character, &patterns)
}

#[cache(LruCache: LruCache::new(*UTF8_MAXIMAL_ALLOCATION))]
pub(crate) fn is_latin(character: &char) -> bool {
    let patterns = ["LATIN"];
    in_description(character, &patterns)
}

#[cache(LruCache: LruCache::new(*UTF8_MAXIMAL_ALLOCATION))]
pub(crate) fn is_cjk(character: &char) -> bool {
    let patterns = ["CJK"];
    in_description(character, &patterns)
}

#[cache(LruCache: LruCache::new(*UTF8_MAXIMAL_ALLOCATION))]
pub(crate) fn is_hiragana(character: &char) -> bool {
    let patterns = ["HIRAGANA"];
    in_description(character, &patterns)
}

#[cache(LruCache: LruCache::new(*UTF8_MAXIMAL_ALLOCATION))]
pub(crate) fn is_katakana(character: &char) -> bool {
    let patterns = ["KATAKANA"];
    in_description(character, &patterns)
}

#[cache(LruCache: LruCache::new(*UTF8_MAXIMAL_ALLOCATION))]
pub(crate) fn is_hangul(character: &char) -> bool {
    let patterns = ["HANGUL"];
    in_description(character, &patterns)
}

#[cache(LruCache: LruCache::new(*UTF8_MAXIMAL_ALLOCATION))]
pub(crate) fn is_thai(character: &char) -> bool {
    let patterns = ["THAI"];
    in_description(character, &patterns)
}

//#[cache(LruCache: LruCache::new(*UTF8_MAXIMAL_ALLOCATION))]
pub(crate) fn is_case_variable(character: &char) -> bool {
    character.is_lowercase() != character.is_uppercase()
}

pub(crate) fn is_unicode_range_secondary(range_name: String) -> bool {
    UNICODE_SECONDARY_RANGE_KEYWORD
        .iter()
        .any(|&s| range_name.contains(s))
}

// Retrieve the Unicode range official name from a single character
//#[cache(LruCache: LruCache::new(*UTF8_MAXIMAL_ALLOCATION))]
pub(crate) fn unicode_range(character: &char) -> Option<&'static str> {
    let char_code = *character as u32;
    for (name, range) in &*UNICODE_RANGES_COMBINED {
        if range.contains(&char_code) {
            return Some(name);
        }
    }
    None
}

pub(crate) fn range_scan(decoded_sequence: &str) -> HashSet<String> {
    let mut result: HashSet<String> = HashSet::new();
    for ch in decoded_sequence.chars() {
        if let Some(r) = unicode_range(&ch) {
            result.insert(r.to_string());
        }
    }
    result
}

//#[cache(LruCache: LruCache::new(*UTF8_MAXIMAL_ALLOCATION))]
pub(crate) fn is_ascii(character: &char) -> bool {
    character.is_ascii()
}

//#[cache(LruCache: LruCache::new(*UTF8_MAXIMAL_ALLOCATION))]
pub(crate) fn remove_accent(ch: &char) -> char {
    let mut base_char = None;
    decompose_canonical(*ch, |c| {
        base_char.get_or_insert(c);
    });
    if let Some(base_char) = base_char {
        return base_char;
    }
    *ch
}

pub(crate) fn should_strip_sig_or_bom(_iana_encoding: &str) -> bool {
    // it looks like we always remove it in Rust (but in Python version no)
    true
}

// Verify is a specific encoding is a multi byte one based on it IANA name
pub fn is_multi_byte_encoding(name: &str) -> bool {
    [
        "utf-8",
        "utf-16le",
        "utf-16be",
        "euc-jp",
        "euc-kr",
        "iso-2022-jp",
        "gbk",
        "gb18030",
        "hz",
        "big5",
        "shift_jis",
    ]
    .contains(&name)
}

// Try to detect multibyte encoding by signature
pub(crate) fn identify_sig_or_bom(sequence: &[u8]) -> (Option<String>, Option<&[u8]>) {
    for (encoding_name, encoding_signature) in &*ENCODING_MARKS {
        if sequence.starts_with(encoding_signature) {
            return (Some(encoding_name.to_string()), Some(encoding_signature));
        }
    }
    (None, None)
}

// Try to get standard name by alternative labels
pub fn iana_name(cp_name: &str) -> Option<&str> {
    // firstly just try to search it in our list
    if IANA_SUPPORTED.contains(&cp_name) {
        return Some(cp_name);
    }
    // if didn't found, try to use alternative way
    if let Some(enc) = encoding_from_whatwg_label(cp_name) {
        return Some(enc.whatwg_name().unwrap_or(enc.name()));
    }
    None
}

pub(crate) fn is_cp_similar(iana_name_a: &str, iana_name_b: &str) -> bool {
    IANA_SUPPORTED_SIMILAR.contains_key(iana_name_a)
        && IANA_SUPPORTED_SIMILAR[iana_name_a].contains(&iana_name_b)
}

// Extract using ASCII-only decoder any specified encoding in the first n-bytes.
pub(crate) fn any_specified_encoding(sequence: &[u8], search_zone: usize) -> Option<String> {
    if let Ok(test_string) = encoding::all::ASCII.decode(
        &sequence[0..search_zone.min(sequence.len())],
        DecoderTrap::Ignore,
    ) {
        for (_, [specified_encoding]) in RE_POSSIBLE_ENCODING_INDICATION
            .captures_iter(&test_string)
            .map(|c| c.extract())
        {
            if let Some(found_iana) = iana_name(specified_encoding) {
                return Some(found_iana.to_string());
            }
        }
    }
    None
}

// Calculate similarity of two single byte encodings
pub(crate) fn cp_similarity(iana_name_a: &str, iana_name_b: &str) -> f32 {
    // we don't want to compare multi-byte encodings
    if is_multi_byte_encoding(iana_name_a) || is_multi_byte_encoding(iana_name_b) {
        return 0.0;
    }

    let mut character_match_count: u8 = 0;
    if let (Some(encoder_a), Some(encoder_b)) = (
        encoding_from_whatwg_label(iana_name_a),
        encoding_from_whatwg_label(iana_name_b),
    ) {
        for i in 1..255 {
            let ch = i as u8;
            if let (Ok(res_a), Ok(res_b)) = (
                encoder_a.decode(&[ch], DecoderTrap::Ignore),
                encoder_b.decode(&[ch], DecoderTrap::Ignore),
            ) {
                if res_a == res_b {
                    character_match_count += 1;
                }
            }
        }
    }
    character_match_count as f32 / 254f32
}

// Test Decoding bytes to string with specified encoding without writing result to memory
// returns true if everything is correctly decoded, otherwise false
struct DecodeTestResult {
    only_test: bool,
    data: String,
}
impl StringWriter for DecodeTestResult {
    fn writer_hint(&mut self, expectedlen: usize) {
        if self.only_test {
            return;
        }
        let newlen = self.data.len() + expectedlen;
        self.data.reserve(newlen);
    }
    fn write_char(&mut self, c: char) {
        if self.only_test {
            return;
        }
        self.data.push(c);
    }
    fn write_str(&mut self, s: &str) {
        if self.only_test {
            return;
        }
        self.data.push_str(s);
    }
}
impl DecodeTestResult {
    pub fn get_buffer(&self) -> &str {
        &self.data
    }
}

// Decode bytes to string with specified encoding
// if is_chunk = true it will try to fix first and end bytes for multibyte encodings
pub fn decode(
    input: &[u8],
    from_encoding: &str,
    how_process_errors: DecoderTrap,
    only_test: bool,
    is_chunk: bool,
) -> Result<String, String> {
    if let Some(encoder) = encoding_from_whatwg_label(from_encoding) {
        let mut buf = DecodeTestResult {
            only_test,
            data: String::new(),
        };
        let mut err = CodecError {
            upto: 0,
            cause: Cow::from(String::new()),
        };
        let chunk_len = input.len();
        let mut begin_offset: usize = 0;
        let mut end_offset: usize = chunk_len;
        let mut res;
        let mut error_occured: bool;
        loop {
            res = decode_to(
                encoder,
                &input[begin_offset..end_offset],
                how_process_errors,
                &mut buf,
            );
            error_occured = res.is_err();
            if let DecoderTrap::Strict = how_process_errors {
            } else {
                break;
            }
            if !is_chunk || res.is_ok() || !is_multi_byte_encoding(from_encoding) {
                break;
            }
            err = res.unwrap_err();
            if err.cause.contains("invalid sequence") {
                begin_offset += 1;
            } else if err.cause.contains("incomplete sequence") {
                end_offset -= 1;
            }
            if end_offset - begin_offset < 1 || begin_offset > 3 || (chunk_len - end_offset) > 3 {
                break;
            }
        }
        if error_occured {
            return Err(format!("{} at index {}", err.cause, err.upto));
        }
        return Ok(String::from(buf.get_buffer()));
    }
    Err(format!("Encoding '{}' not found", from_encoding))
}

// Copied implementation of decode_to from encoder lib
// (we need index of problematic chars & hacks for chunks)
fn decode_to(
    encoder: EncodingRef,
    input: &[u8],
    trap: DecoderTrap,
    ret: &mut dyn StringWriter,
) -> Result<(), CodecError> {
    let mut decoder = encoder.raw_decoder();
    let mut remaining = 0;
    loop {
        let (offset, err) = decoder.raw_feed(&input[remaining..], ret);
        let unprocessed = remaining + offset;
        match err {
            Some(err) => {
                remaining = (remaining as isize + err.upto) as usize;
                if !trap.trap(&mut *decoder, &input[unprocessed..remaining], ret) {
                    return Err(err);
                }
            }
            None => {
                remaining = input.len();
                if let Some(err) = decoder.raw_finish(ret) {
                    remaining = (remaining as isize + err.upto) as usize;
                    if !trap.trap(&mut *decoder, &input[unprocessed..remaining], ret) {
                        return Err(err);
                    }
                }
                if remaining >= input.len() {
                    return Ok(());
                }
            }
        }
    }
}

// Round float to specified precision
pub fn round_float(val: f32, precision: u8) -> f32 {
    let mult = 10.0f32.powf(precision as f32);
    (val * mult).round() / mult
}

// Encode string to vec of bytes with specified encoding
pub fn encode(
    input: &str,
    to_encoding: &str,
    how_process_errors: EncoderTrap,
) -> Result<Vec<u8>, String> {
    if let Some(encoder) = encoding_from_whatwg_label(to_encoding) {
        return Ok(encoder.encode(input, how_process_errors)?);
    }
    Err(format!("Encoding '{}' not found", to_encoding))
}

// Determine if two Unicode range seen next to each other can be considered as suspicious.
#[cache(LruCache: LruCache::new(1024))]
pub(crate) fn is_suspiciously_successive_range(
    range_a: Option<&'static str>,
    range_b: Option<&'static str>,
) -> bool {
    // both arguments should not be None
    if [range_a, range_b].iter().any(|x| x.is_none()) {
        return true;
    }
    let range_a = range_a.unwrap();
    let range_b = range_b.unwrap();

    // some edge cases
    if range_a == range_b
        || [range_a, range_b].iter().all(|x| x.contains("Latin"))
        || [range_a, range_b].iter().any(|x| x.contains("Emoticons"))
    {
        return false;
    }

    // Latin characters can be accompanied with a combining diacritical mark
    // eg. Vietnamese.
    if [range_a, range_b].iter().any(|x| x.contains("Latin"))
        && [range_a, range_b].iter().any(|x| x.contains("Combining"))
    {
        return false;
    }

    // keywords intersection
    let set_a: HashSet<_> = range_a.split_whitespace().collect();
    let set_b: HashSet<_> = range_b.split_whitespace().collect();

    let common: HashSet<_> = set_a.intersection(&set_b).cloned().collect();
    if common
        .difference(&*UNICODE_SECONDARY_RANGE_KEYWORD)
        .next()
        .is_some()
    {
        return false;
    }

    // Japanese exception
    let jp_ranges = ["Hiragana", "Katakana"];
    let jp_a = jp_ranges.contains(&range_a);
    let jp_b = jp_ranges.contains(&range_b);
    let has_cjk = range_a.contains("CJK") || range_b.contains("CJK");

    if (jp_a || jp_b) && has_cjk {
        return false;
    }

    if jp_a && jp_b {
        return false;
    }

    if [range_a, range_b].iter().any(|x| x.contains("Hangul")) {
        if has_cjk {
            return false;
        }
        if [range_a, range_b].iter().any(|x| *x == "Basic Latin") {
            return false;
        }
    }

    // Chinese use dedicated range for punctuation and/or separators.
    if has_cjk
        && [range_a, range_b]
            .iter()
            .any(|x| x.contains("Punctuation") || x.contains("Forms"))
    {
        return false;
    }

    true
}

// Get data for specified language
pub(crate) fn get_language_data(language: &Language) -> Result<(&'static str, bool, bool), String> {
    for (iterated_language, characters, has_accents, pure_latin) in LANGUAGES.iter() {
        if iterated_language == language {
            return Ok((characters, *has_accents, *pure_latin));
        }
    }
    Err(String::from("Language wasn't found"))
}

// Concatenate &[u8]
pub(crate) fn concatenate_slices<'a>(slice1: &'a [u8], slice2: &'a [u8]) -> Vec<u8> {
    let mut result = Vec::with_capacity(slice1.len() + slice2.len());
    result.extend_from_slice(slice1);
    result.extend_from_slice(slice2);
    result
}

// Get large datasets
fn collect_large_sets(dir: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();

    if dir.is_dir() {
        for entry in fs::read_dir(dir).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();

            if path.is_dir() {
                // Recursively collect files in subdirectories
                let subdirectory_files = collect_large_sets(&path);
                files.extend(subdirectory_files);
            } else {
                // Add the file to the list if it's a regular file
                files.push(path);
            }
        }
    }
    files
}

// Get large datasets
pub fn get_large_test_datasets() -> Result<Vec<(String, Vec<String>)>, String> {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.push("src/tests/data/largesets/");

    if fs::metadata(&path).is_err() || !fs::metadata(&path).unwrap().is_dir() {
        return Err(format!("Cannot to find large datasets at {:?}", &path));
    }

    let large_sets = collect_large_sets(&path);
    let mut result: Vec<(String, Vec<String>)> = vec![];
    for set in &large_sets {
        let path = set.to_str().unwrap();
        let encoding: Vec<&str> = path.split('/').collect();
        let encoding: Vec<String> = encoding[encoding.len() - 2]
            .split(',')
            .map(|s| s.to_string())
            .collect();
        if encoding.len() == 1 && encoding.first().unwrap() == "largesets" {
            continue;
        }
        result.push((path.to_string(), encoding));
    }
    Ok(result)
}