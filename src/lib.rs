use crate::cd::{
    coherence_ratio, encoding_languages, mb_encoding_languages, merge_coherence_ratios,
};
use crate::consts::{IANA_SUPPORTED, MAX_PROCESSED_BYTES, TOO_BIG_SEQUENCE, TOO_SMALL_SEQUENCE};
use crate::entity::{CharsetMatch, CharsetMatches, CoherenceMatches, NormalizerSettings};
use crate::md::mess_ratio;
use crate::utils::{
    any_specified_encoding, concatenate_slices, decode, iana_name, identify_sig_or_bom,
    is_cp_similar, is_multi_byte_encoding, round_float, should_strip_sig_or_bom,
};
use encoding::DecoderTrap;
use log::{debug, trace};
use std::fs::File;
use std::io::Read;
use std::path::PathBuf;

pub mod assets;
mod cd;
pub mod consts;
pub mod entity;
mod md;
mod tests;
pub mod utils;

// Given a raw bytes sequence, return the best possibles charset usable to render str objects.
// If there is no results, it is a strong indicator that the source is binary/not text.
// By default, the process will extract 5 blocks of 512o each to assess the mess and coherence of a given sequence.
// And will give up a particular code page after 20% of measured mess. Those criteria are customizable at will.
//
// The preemptive behavior DOES NOT replace the traditional detection workflow, it prioritize a particular code page
// but never take it for granted. Can improve the performance.
//
// You may want to focus your attention to some code page or/and not others, use cp_isolation and cp_exclusion for that
// purpose.
//
// This function will strip the SIG in the payload/sequence every time except on UTF-16, UTF-32.
// By default the library does not setup any handler other than the NullHandler, if you choose to set the 'explain'
// toggle to True it will alter the logger configuration to add a StreamHandler that is suitable for debugging.
// Custom logging format and handler can be set manually.
pub fn from_bytes(bytes: &Vec<u8>, settings: Option<NormalizerSettings>) -> CharsetMatches {
    // init settings with default values if it's None and recheck include_encodings and
    // exclude_encodings settings
    let mut settings = settings.unwrap_or(NormalizerSettings::default());
    if !settings.include_encodings.is_empty() {
        settings.include_encodings = settings
            .include_encodings
            .iter()
            .map(|e| iana_name(e).unwrap().to_string())
            .collect();
        trace!(
            "include_encodings is set. Use this flag for debugging purpose. \
        Limited list of encoding allowed : {}.",
            settings.include_encodings.join(", ")
        );
    }
    if !settings.exclude_encodings.is_empty() {
        settings.exclude_encodings = settings
            .exclude_encodings
            .iter()
            .map(|e| iana_name(e).unwrap().to_string())
            .collect();
        trace!(
            "exclude_encodings is set. Use this flag for debugging purpose. \
        Limited list of encoding allowed : {}.",
            settings.exclude_encodings.join(", ")
        );
    }

    // check for empty
    let bytes_length = bytes.len();
    if bytes_length == 0 {
        debug!("Encoding detection on empty bytes, assuming utf_8 intention.");
        return CharsetMatches::new(Some(vec![CharsetMatch::new(
            bytes,
            "utf-8",
            0.0,
            false,
            &vec![],
            None,
        )]));
    }

    // check min length
    if bytes_length <= (settings.chunk_size * settings.steps) {
        trace!(
            "override steps ({}) and chunk_size ({}) as content does not \
            fit ({} byte(s) given) parameters.",
            settings.steps,
            settings.chunk_size,
            bytes_length,
        );
        settings.steps = 1;
        settings.chunk_size = bytes_length;
    }

    if settings.steps > 1 && bytes_length / settings.steps < settings.chunk_size {
        settings.chunk_size = bytes_length / settings.steps;
    }

    // too small length
    if bytes_length < *TOO_SMALL_SEQUENCE {
        trace!(
            "Trying to detect encoding from a tiny portion of ({}) byte(s).",
            bytes_length
        );
    }

    // too big length
    let is_too_large_sequence = bytes_length > *TOO_BIG_SEQUENCE;
    if is_too_large_sequence {
        trace!(
            "Using lazy str decoding because the payload is quite large, ({}) byte(s).",
            bytes_length
        );
    }

    // start to build prioritized encodings array
    let mut prioritized_encodings: Vec<String> = vec![];

    // search for encoding in the content
    let mut specified_encoding: String = String::new();
    if settings.preemptive_behaviour {
        if let Some(enc) = any_specified_encoding(bytes, 4096) {
            trace!(
                "Detected declarative mark in sequence. Priority +1 given for {}.",
                &enc
            );
            specified_encoding = enc.clone();
            prioritized_encodings.push(enc);
        }
    }

    // check bom & sig
    let (sig_encoding, sig_payload) = identify_sig_or_bom(bytes);
    if sig_encoding.is_some() {
        trace!(
            "Detected a SIG or BOM mark on first {} byte(s). Priority +1 given for {}.",
            sig_payload.unwrap().len(),
            &sig_encoding.clone().unwrap(),
        );
        prioritized_encodings.push(sig_encoding.clone().unwrap());
    }

    // add ascii & utf-8
    for enc in &["ascii", "utf-8"] {
        prioritized_encodings.push(enc.to_string());
    }

    // generate array of encodings for probing with prioritizing
    let mut iana_encodings = IANA_SUPPORTED.clone();
    for pe in prioritized_encodings.iter().rev() {
        if let Some(index) = iana_encodings.iter().position(|x| *x == pe) {
            let value = iana_encodings.remove(index);
            iana_encodings.insert(0, value);
        }
    }

    // Main processing loop variables
    let mut tested_but_hard_failure: Vec<&str> = vec![];
    let mut tested_but_soft_failure: Vec<&str> = vec![];
    let mut fallback_ascii: Option<CharsetMatch> = None;
    let mut fallback_u8: Option<CharsetMatch> = None;
    let mut fallback_specified: Option<CharsetMatch> = None;
    let mut results: CharsetMatches = CharsetMatches::new(None);

    // Iterate and probe our encodings
    'iana_encodings_loop: for encoding_iana in iana_encodings {
        if !settings.include_encodings.is_empty()
            && !settings
                .include_encodings
                .contains(&encoding_iana.to_string())
        {
            continue;
        }
        if settings
            .exclude_encodings
            .contains(&encoding_iana.to_string())
        {
            continue;
        }
        let bom_or_sig_available: bool = sig_encoding == Some(encoding_iana.to_string());
        let strip_sig_or_bom: bool = bom_or_sig_available && should_strip_sig_or_bom(encoding_iana);
        let is_multi_byte_decoder: bool = is_multi_byte_encoding(encoding_iana);

        // utf-16le & utf-16be cannot be identified without BOM
        if !bom_or_sig_available && ["utf-16le", "utf-16be"].contains(&encoding_iana) {
            trace!(
                "Encoding {} won't be tested as-is because it require a BOM. Will try some sub-encoder LE/BE",
                encoding_iana,
            );
            continue;
        }

        // fast pre-check
        let mut decoded_payload: Option<&str> = None;
        let decoded_payload_result = decode(
            &bytes[if strip_sig_or_bom {
                sig_payload.unwrap().len()
            } else {
                0
            }..if is_too_large_sequence && !is_multi_byte_decoder {
                *MAX_PROCESSED_BYTES
            } else {
                bytes_length
            }],
            encoding_iana,
            DecoderTrap::Strict,
            is_too_large_sequence && !is_multi_byte_decoder,
            false,
        );
        if decoded_payload_result.is_ok() {
            if !is_too_large_sequence || is_multi_byte_decoder {
                decoded_payload = Some(decoded_payload_result.as_ref().unwrap());
            }
        } else {
            trace!(
                "Code page {} does not fit given bytes sequence at ALL.",
                encoding_iana,
            );
            tested_but_hard_failure.push(encoding_iana);
            continue 'iana_encodings_loop;
        }

        // soft failed pre-check
        // important thing! it occurs sometimes fail detection
        for encoding_soft_failed in &tested_but_soft_failure {
            if is_cp_similar(encoding_iana, encoding_soft_failed) {
                trace!("{} is deemed too similar to code page {} and was consider unsuited already. Continuing!",
                    encoding_iana,
                    encoding_soft_failed,
                );
                continue 'iana_encodings_loop;
            }
        }

        // lets split input by chunks and try to parse them
        let max_chunk_gave_up = 2.max(settings.steps / 4);
        let mut early_stop_count: usize = 0;
        let mut lazy_str_hard_failure = false;
        let mut md_ratios: Vec<f32> = vec![];

        // detect target languages
        let target_languages = if is_multi_byte_decoder {
            mb_encoding_languages(encoding_iana)
        } else {
            encoding_languages(encoding_iana.to_string())
        };
        trace!(
            "{} should target any language(s) of {:?}",
            encoding_iana,
            target_languages,
        );

        // main loop over chunks in our input
        // we go over bytes or chars - it depends on previous code
        let sequence_length = if decoded_payload.is_some() {
            decoded_payload.as_ref().unwrap_or(&"").chars().count()
        } else {
            bytes_length
        };
        let offsets = ((if bom_or_sig_available && decoded_payload.is_none() {
            sig_payload.unwrap().len()
        } else {
            0
        })..sequence_length)
            .step_by((sequence_length / settings.steps).max(1));

        // Chunks Loop
        // Iterate over chunks of bytes or chars
        let mut md_chunks: Vec<String> = vec![];
        'chunks_loop: for offset in offsets {
            let decoded_chunk_result = if decoded_payload.is_some() {
                // Chars processing
                Ok(decoded_payload
                    .as_ref()
                    .unwrap_or(&"")
                    .chars()
                    .skip(offset)
                    .take(settings.chunk_size)
                    .collect())
            } else {
                // Bytes processing
                let offset_end = (offset + settings.chunk_size).min(sequence_length);
                let cut_bytes_vec = if bom_or_sig_available && !strip_sig_or_bom {
                    sig_payload.unwrap()
                } else {
                    &[]
                };
                let cut_bytes_vec = concatenate_slices(cut_bytes_vec, &bytes[offset..offset_end]);
                let cut_bytes = cut_bytes_vec.as_slice();
                decode(cut_bytes, encoding_iana, DecoderTrap::Strict, false, false)
            };

            // ascii in encodings means windows-1252 codepage with supports diacritis
            // because of this we will check additionally it with is_ascii method
            if decoded_chunk_result.is_err()
                || (encoding_iana == "ascii" && !decoded_chunk_result.as_ref().unwrap().is_ascii())
            {
                trace!(
                    "LazyStr Loading: After MD chunk decode, code page {} \
                    does not fit given bytes sequence at ALL. {}",
                    encoding_iana,
                    if decoded_chunk_result.is_err() {
                        decoded_chunk_result.unwrap_err().to_string()
                    } else {
                        String::from("non-ascii")
                    },
                );
                early_stop_count = max_chunk_gave_up;
                lazy_str_hard_failure = true;
                break 'chunks_loop;
            }
            let decoded_chunk = decoded_chunk_result.as_ref().unwrap();

            // MD ratios calc
            md_chunks.push(decoded_chunk.to_string());
            md_ratios.push(mess_ratio(
                decoded_chunk.to_string(),
                Some(settings.threshold),
            ));
            if md_ratios.last().unwrap() >= &settings.threshold {
                early_stop_count += 1;
            }
            if early_stop_count >= max_chunk_gave_up || (bom_or_sig_available && !strip_sig_or_bom)
            {
                break 'chunks_loop;
            }
        }

        // We might want to check the remainder of sequence
        // Only if initial MD tests passes
        if !lazy_str_hard_failure && is_too_large_sequence && !is_multi_byte_decoder {
            let decoded_chunk_result = decode(
                &bytes[*MAX_PROCESSED_BYTES..],
                encoding_iana,
                DecoderTrap::Strict,
                false,
                false,
            );
            if decoded_chunk_result.is_err()
                || (encoding_iana == "ascii" && !decoded_chunk_result.as_ref().unwrap().is_ascii())
            {
                trace!(
                    "LazyStr Loading: After final lookup, code page {} does not fit \
                    given bytes sequence at ALL. {}",
                    encoding_iana,
                    decoded_chunk_result.unwrap_err().to_string(),
                );
                tested_but_hard_failure.push(encoding_iana);
                continue 'iana_encodings_loop;
            }
        }

        // process mean mess ratio
        let mean_mess_ratio = if md_ratios.is_empty() {
            0.0
        } else {
            md_ratios.iter().sum::<f32>() / (md_ratios.len() as f32)
        };
        let mean_mess_ratio_percent = round_float(mean_mess_ratio * 100.0, 3);

        if mean_mess_ratio >= *settings.threshold || early_stop_count >= max_chunk_gave_up {
            tested_but_soft_failure.push(encoding_iana);
            trace!(
                "{} was excluded because of initial chaos probing. \
                Gave up {} time(s). Computed mean chaos is {} %.",
                encoding_iana,
                early_stop_count,
                mean_mess_ratio_percent,
            );
            // Preparing those fallbacks in case we got nothing.
            if settings.enable_fallback
                && !lazy_str_hard_failure
                && prioritized_encodings.contains(&encoding_iana.to_string())
            {
                let fallback_entry = Some(CharsetMatch::new(
                    bytes,
                    encoding_iana,
                    f32::from(settings.threshold),
                    false,
                    &vec![],
                    decoded_payload,
                ));

                if encoding_iana == specified_encoding {
                    fallback_specified = fallback_entry;
                } else if encoding_iana == "ascii" {
                    fallback_ascii = fallback_entry;
                } else {
                    fallback_u8 = fallback_entry;
                }
            }
            continue 'iana_encodings_loop;
        }
        trace!(
            "{} passed initial chaos probing. Mean measured chaos is {} %",
            encoding_iana,
            mean_mess_ratio_percent,
        );

        // CD rations calc
        // We shall skip the CD when its about ASCII
        // Most of the time its not relevant to run "language-detection" on it.
        let mut cd_ratios: Vec<CoherenceMatches> = vec![];
        if encoding_iana != "ascii" {
            for chunk in &md_chunks {
                if let Ok(chunk_coherence_matches) = coherence_ratio(
                    chunk.to_string(),
                    Some(settings.language_threshold),
                    Some(target_languages.clone()),
                ) {
                    cd_ratios.push(chunk_coherence_matches);
                }
            }
        }

        // process cd ratios
        let cd_ratios_merged = merge_coherence_ratios(&cd_ratios);
        if !cd_ratios_merged.is_empty() {
            trace!(
                "We detected language {:?} using {}",
                cd_ratios_merged,
                encoding_iana
            );
        }

        // process results
        results.append(CharsetMatch::new(
            bytes,
            encoding_iana,
            mean_mess_ratio,
            bom_or_sig_available,
            &cd_ratios_merged,
            decoded_payload,
        ));

        if (mean_mess_ratio < 0.1 && prioritized_encodings.contains(&encoding_iana.to_string()))
            || encoding_iana == sig_encoding.clone().unwrap_or(String::new())
        {
            debug!(
                "Encoding detection: {} is most likely the one.",
                encoding_iana
            );
            return CharsetMatches::new(Some(vec![results
                .get_by_encoding(encoding_iana)
                .unwrap()
                .clone()]));
        }
    }

    // fallbacks
    if results.is_empty() {
        let mut fb: Option<&CharsetMatch> = None;
        if fallback_specified.is_some() {
            fb = Some(fallback_specified.as_ref().unwrap());
        } else if fallback_u8.is_some()
            && (fallback_ascii.is_none()
                || (fallback_ascii.is_some()
                    && fallback_u8.as_ref().unwrap().fingerprint()
                        != fallback_ascii.as_ref().unwrap().fingerprint()))
        {
            fb = Some(fallback_u8.as_ref().unwrap());
        } else if fallback_ascii.is_some() {
            fb = Some(fallback_ascii.as_ref().unwrap());
        }
        if let Some(fb_to_pass) = fb {
            debug!(
                "Encoding detection: will be used as a fallback match {}",
                fb_to_pass.encoding()
            );
            results.append(fb_to_pass.clone());
        }
    }

    // final logger information
    if results.is_empty() {
        debug!("Encoding detection: Unable to determine any suitable charset.");
    } else {
        debug!(
            "Encoding detection: Found {} as plausible (best-candidate) for content. \
            With {} alternatives.",
            results.get_best().unwrap().encoding(),
            results.len() - 1,
        );
    }
    results
}

// Same thing than the function from_bytes but with one extra step.
// Opening and reading given file path in binary mode.
// Can return Error.
pub fn from_path(
    path: &PathBuf,
    settings: Option<NormalizerSettings>,
) -> Result<CharsetMatches, String> {
    // read file
    let file = File::open(path);
    if file.is_err() {
        return Err(String::from("Error opening file"));
    }

    let mut buffer = Vec::new();
    if file.unwrap().read_to_end(&mut buffer).is_err() {
        return Err(String::from("Error reading from file"));
    }

    // calculate
    Ok(from_bytes(&buffer, settings))
}