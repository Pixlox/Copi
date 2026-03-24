pub trait OcrEngine: Send + Sync {
    fn recognize_text(&self, image_data: &[u8], width: u32, height: u32) -> Result<String, String>;
}

pub fn init_ocr_engine() -> Result<Box<dyn OcrEngine>, String> {
    #[cfg(target_os = "macos")]
    {
        eprintln!("[OCR] Initializing Apple Vision engine");
        let supported_languages = load_supported_languages();
        let passes = build_ocr_passes(supported_languages.as_ref());

        if passes.is_empty() {
            return Err("Apple Vision returned no supported OCR languages".into());
        }

        if let Some(supported) = supported_languages {
            eprintln!(
                "[OCR] Vision supports {} languages; configured {} OCR passes",
                supported.len(),
                passes.len()
            );
        }

        Ok(Box::new(AppleVisionOcr { passes }))
    }

    #[cfg(not(target_os = "macos"))]
    {
        Err("OCR not available on this platform".to_string())
    }
}

#[cfg(target_os = "macos")]
use std::collections::HashSet;

#[cfg(target_os = "macos")]
use std::{ffi::c_void, ptr::null_mut};

#[cfg(target_os = "macos")]
use cidre::{
    cv::{PixelBuf, PixelFormat},
    ns,
    vn::{self, ImageRequestHandler},
};

#[cfg(target_os = "macos")]
struct AppleVisionOcr {
    passes: Vec<OcrPass>,
}

#[cfg(target_os = "macos")]
impl OcrEngine for AppleVisionOcr {
    fn recognize_text(&self, image_data: &[u8], width: u32, height: u32) -> Result<String, String> {
        let grayscale = rgba_to_grayscale(image_data, width, height)?;
        run_vision_ocr(&grayscale, width as usize, height as usize, &self.passes)
    }
}

#[cfg(target_os = "macos")]
#[derive(Clone, Debug)]
struct OcrPass {
    name: &'static str,
    languages: Vec<String>,
    use_language_correction: bool,
}

#[cfg(target_os = "macos")]
#[derive(Clone, Debug)]
struct OcrPassResult {
    text: String,
    avg_confidence: f32,
    char_count: usize,
    meaningful_char_count: usize,
}

#[cfg(target_os = "macos")]
const PRIMARY_LANGUAGE_GROUPS: &[&[&str]] = &[
    &["en-US", "en-GB", "en"],
    &["es-ES", "es-MX", "es"],
    &["fr-FR", "fr"],
    &["de-DE", "de"],
    &["it-IT", "it"],
    &["pt-BR", "pt-PT", "pt"],
    &["nl-NL", "nl"],
    &["da-DK", "da"],
    &["sv-SE", "sv"],
    &["nb-NO", "nn-NO", "no-NO", "nb", "nn", "no"],
    &["ja-JP", "ja"],
    &["ko-KR", "ko"],
    &["zh-Hans", "zh-Hant", "zh-CN", "zh-TW", "zh"],
    &["ru-RU", "ru"],
    &["uk-UA", "uk"],
    &["ar-SA", "ar"],
    &["th-TH", "th"],
    &["vi-VN", "vi"],
];

const LATIN_CORRECTION_GROUPS: &[&[&str]] = &[
    &["en-US", "en-GB", "en"],
    &["es-ES", "es-MX", "es"],
    &["fr-FR", "fr"],
    &["de-DE", "de"],
    &["it-IT", "it"],
    &["pt-BR", "pt-PT", "pt"],
    &["nl-NL", "nl"],
    &["da-DK", "da"],
    &["sv-SE", "sv"],
    &["nb-NO", "nn-NO", "no-NO", "nb", "nn", "no"],
    &["tr-TR", "tr"],
];

#[cfg(target_os = "macos")]
#[no_mangle]
extern "C" fn release_callback(_refcon: *mut c_void, _data_ptr: *const *const c_void) {}

#[cfg(target_os = "macos")]
fn rgba_to_grayscale(data: &[u8], width: u32, height: u32) -> Result<Vec<u8>, String> {
    let expected_len = (width as usize)
        .checked_mul(height as usize)
        .and_then(|pixels| pixels.checked_mul(4))
        .ok_or("OCR image dimensions overflow")?;

    if data.len() < expected_len {
        return Err(format!(
            "OCR image buffer too small: expected at least {} bytes, got {}",
            expected_len,
            data.len()
        ));
    }

    let mut grayscale = Vec::with_capacity((width as usize) * (height as usize));
    for pixel in data[..expected_len].chunks_exact(4) {
        let luminance = ((299u32 * pixel[0] as u32)
            + (587u32 * pixel[1] as u32)
            + (114u32 * pixel[2] as u32)
            + 500)
            / 1000;
        grayscale.push(luminance as u8);
    }

    Ok(grayscale)
}

#[cfg(target_os = "macos")]
fn load_supported_languages() -> Option<HashSet<String>> {
    cidre::objc::ar_pool(|| {
        let mut request = vn::RecognizeTextRequest::new();
        request.set_recognition_level(vn::RequestTextRecognitionLevel::Accurate);
        request
            .supported_recognition_langs()
            .ok()
            .map(|languages| languages.iter().map(|lang| lang.to_string()).collect())
    })
}

#[cfg(target_os = "macos")]
fn build_ocr_passes(supported_languages: Option<&HashSet<String>>) -> Vec<OcrPass> {
    let multilingual_languages = collect_supported_languages(supported_languages, PRIMARY_LANGUAGE_GROUPS);
    let latin_languages = collect_supported_languages(supported_languages, LATIN_CORRECTION_GROUPS);
    let mut passes = Vec::new();
    if !multilingual_languages.is_empty() {
        passes.push(OcrPass {
            name: "multilingual",
            languages: multilingual_languages,
            use_language_correction: false,
        });
    }
    if !latin_languages.is_empty() {
        passes.push(OcrPass {
            name: "latin_corrected",
            languages: latin_languages,
            use_language_correction: true,
        });
    }
    passes
}

#[cfg(target_os = "macos")]
fn collect_supported_languages(
    supported_languages: Option<&HashSet<String>>,
    groups: &[&[&str]],
) -> Vec<String> {
    let mut selected = Vec::new();
    for group in groups {
        if let Some(language) = select_supported_language(supported_languages, group) {
            if !selected.iter().any(|existing| existing == &language) {
                selected.push(language);
            }
        }
    }
    selected
}

#[cfg(target_os = "macos")]
fn select_supported_language(
    supported_languages: Option<&HashSet<String>>,
    candidates: &[&str],
) -> Option<String> {
    match supported_languages {
        Some(supported) => {
            for candidate in candidates {
                if supported.contains(*candidate) {
                    return Some((*candidate).to_string());
                }
            }

            for candidate in candidates {
                let prefix = candidate.split('-').next().unwrap_or(candidate);
                if let Some(language) = supported.iter().find(|language| {
                    *language == prefix
                        || language
                            .strip_prefix(prefix)
                            .is_some_and(|suffix| suffix.starts_with('-'))
                }) {
                    return Some(language.clone());
                }
            }

            None
        }
        None => candidates.first().map(|candidate| (*candidate).to_string()),
    }
}

#[cfg(target_os = "macos")]
fn run_vision_ocr(
    grayscale: &[u8],
    width: usize,
    height: usize,
    passes: &[OcrPass],
) -> Result<String, String> {
    cidre::objc::ar_pool(|| {
        let mut pixel_buf_out = None;
        let pixel_buf = unsafe {
            PixelBuf::create_with_bytes_in(
                width,
                height,
                PixelFormat::ONE_COMPONENT_8,
                grayscale.as_ptr() as *mut c_void,
                width,
                release_callback,
                null_mut(),
                None,
                &mut pixel_buf_out,
                None,
            )
            .to_result_unchecked(pixel_buf_out)
        }
        .map_err(|error| format!("Failed to create OCR pixel buffer: {error:?}"))?;

        let handler = ImageRequestHandler::with_cv_pixel_buf(&pixel_buf, None)
            .ok_or("Failed to create Vision request handler".to_string())?;

        let mut best_result: Option<OcrPassResult> = None;
        let mut last_error: Option<String> = None;

        for pass in passes {
            match run_vision_pass(&handler, pass) {
                Ok(result) => {
                    if result.text.trim().is_empty() {
                        continue;
                    }
                    if result.meaningful_char_count == 0 {
                        continue;
                    }

                    let score = ocr_result_score(&result);
                    let replace = best_result
                        .as_ref()
                        .map(|best| score > ocr_result_score(best))
                        .unwrap_or(true);

                    if replace {
                        best_result = Some(result);
                    }

                    if best_result
                        .as_ref()
                        .is_some_and(|best| best.avg_confidence >= 0.86 && best.meaningful_char_count >= 24)
                    {
                        break;
                    }
                }
                Err(error) => {
                    eprintln!("[OCR] Pass {} failed: {}", pass.name, error);
                    last_error = Some(error);
                }
            }
        }

        if let Some(result) = best_result {
            eprintln!(
                "[OCR] Selected result: {} chars, confidence {:.3}",
                result.char_count, result.avg_confidence
            );
            Ok(result.text)
        } else if let Some(error) = last_error {
            Err(error)
        } else {
            Ok(String::new())
        }
    })
}

#[cfg(target_os = "macos")]
fn ocr_result_score(result: &OcrPassResult) -> f32 {
    result.avg_confidence * result.meaningful_char_count as f32
}

#[cfg(target_os = "macos")]
fn run_vision_pass(handler: &ImageRequestHandler, pass: &OcrPass) -> Result<OcrPassResult, String> {
    let mut request = vn::RecognizeTextRequest::new();
    request.set_recognition_level(vn::RequestTextRecognitionLevel::Accurate);
    request.set_uses_lang_correction(pass.use_language_correction);

    let mut languages_array = ns::ArrayMut::<ns::String>::with_capacity(pass.languages.len());
    for language in &pass.languages {
        languages_array.push(&ns::String::with_str(language));
    }
    request.set_recognition_langs(&languages_array);

    let requests = ns::Array::<vn::Request>::from_slice(&[&request]);
    handler
        .perform(&requests)
        .map_err(|error| format!("Vision request failed: {error:?}"))?;

    extract_result(&request)
}

#[cfg(target_os = "macos")]
fn extract_result(request: &vn::RecognizeTextRequest) -> Result<OcrPassResult, String> {
    let Some(results) = request.results() else {
        return Ok(OcrPassResult {
            text: String::new(),
            avg_confidence: 0.0,
            char_count: 0,
            meaningful_char_count: 0,
        });
    };

    let mut text = String::new();
    let mut confidence_total = 0.0f32;
    let mut confidence_count = 0usize;

    for observation in results.iter() {
        let candidates = observation.top_candidates(1);
        let Ok(candidate) = candidates.get(0) else {
            continue;
        };

        let recognized = candidate.string().to_string();
        if recognized.trim().is_empty() {
            continue;
        }

        if !text.is_empty() {
            text.push('\n');
        }
        text.push_str(recognized.trim());

        confidence_total += candidate.confidence();
        confidence_count += 1;
    }

    let char_count = text.chars().count();
    let meaningful_char_count = text.chars().filter(|ch| ch.is_alphanumeric()).count();
    let avg_confidence = if confidence_count == 0 {
        0.0
    } else {
        confidence_total / confidence_count as f32
    };

    Ok(OcrPassResult {
        text,
        avg_confidence,
        char_count,
        meaningful_char_count,
    })
}
