/// OCR trait — designed for cross-platform extensibility.
/// macOS: Apple Vision (VNRecognizeTextRequest)
/// Windows: Tesseract or RapidOCR (future)

pub trait OcrEngine: Send + Sync {
    fn recognize_text(&self, image_data: &[u8], width: u32, height: u32) -> Result<String, String>;
}

/// Initialize the OCR engine for the current platform.
pub fn init_ocr_engine() -> Result<Box<dyn OcrEngine>, String> {
    #[cfg(target_os = "macos")]
    {
        eprintln!("[OCR] Initializing Apple Vision engine");
        Ok(Box::new(AppleVisionOcr::new()?))
    }

    #[cfg(not(target_os = "macos"))]
    {
        eprintln!("[OCR] No OCR engine available for this platform");
        Err("OCR not available on this platform".to_string())
    }
}

// ─── Apple Vision Implementation (macOS) ───────────────────────────

#[cfg(target_os = "macos")]
struct AppleVisionOcr;

#[cfg(target_os = "macos")]
impl AppleVisionOcr {
    fn new() -> Result<Self, String> {
        Ok(AppleVisionOcr)
    }
}

#[cfg(target_os = "macos")]
impl OcrEngine for AppleVisionOcr {
    fn recognize_text(&self, image_data: &[u8], width: u32, height: u32) -> Result<String, String> {
        use objc::runtime::{Class, Object};
        use objc::{msg_send, sel, sel_impl};

        unsafe {
            // Create CGImage from raw RGBA pixel data
            let data_len = image_data.len();
            let color_space: *mut Object = msg_send![Class::get("CGColorSpace").unwrap(), colorSpaceWithName: {
                let name: *mut Object = msg_send![Class::get("NSString").unwrap(), stringWithUTF8String: b"kCGColorSpaceSRGB\0".as_ptr()];
                name
            }];

            let provider: *mut Object = msg_send![
                Class::get("CGDataProvider").unwrap(),
                createWithData: std::ptr::null::<u8>()
                    data: image_data.as_ptr()
                    size: data_len
            ];

            // CGImageCreate
            let cg_image: *mut Object = msg_send![
                Class::get("CGImage").unwrap(),
                createWithWidth: width as usize
                height: height as usize
                bitsPerComponent: 8
                bitsPerPixel: 32
                bytesPerRow: (width as usize * 4)
                space: color_space
                bitmapInfo: 1u32 // kCGImageAlphaLast
                provider: provider
                decode: std::ptr::null::<u8>()
                shouldInterpolate: false
                intent: 0i64
            ];

            if cg_image.is_null() {
                return Err("Failed to create CGImage".to_string());
            }

            // Create VNImageRequestHandler
            let handler_class = Class::get("VNImageRequestHandler")
                .ok_or("VNImageRequestHandler class not found")?;
            let handler: *mut Object = msg_send![handler_class, alloc];
            let handler: *mut Object = msg_send![handler, initWithCGImage: cg_image options: {
                let empty_dict: *mut Object = msg_send![Class::get("NSDictionary").unwrap(), dictionary];
                empty_dict
            }];

            // Create VNRecognizeTextRequest
            let request_class = Class::get("VNRecognizeTextRequest")
                .ok_or("VNRecognizeTextRequest class not found")?;
            let request: *mut Object = msg_send![request_class, alloc];
            let request: *mut Object = msg_send![request, init];

            // Set recognition level to accurate
            let _: () = msg_send![request, setRecognitionLevel: 1i64]; // VNRequestTextRecognitionLevelAccurate

            // Perform the request
            let mut error: *mut Object = std::ptr::null_mut();
            let requests: *mut Object =
                msg_send![Class::get("NSArray").unwrap(), arrayWithObject: request];
            let success: bool = msg_send![handler, performRequests: requests error: &mut error];

            if !success {
                return Err("OCR request failed".to_string());
            }

            // Get results
            let results: *mut Object = msg_send![request, results];
            if results.is_null() {
                return Ok(String::new());
            }

            let count: usize = msg_send![results, count];
            let mut text = String::new();

            for i in 0..count {
                let observation: *mut Object = msg_send![results, objectAtIndex: i];
                let top_candidates: *mut Object = msg_send![observation, topCandidates: 1usize];
                if !top_candidates.is_null() {
                    let candidate_count: usize = msg_send![top_candidates, count];
                    if candidate_count > 0 {
                        let candidate: *mut Object = msg_send![top_candidates, objectAtIndex: 0];
                        let string: *const Object = msg_send![candidate, string];
                        let c_str: *const std::os::raw::c_char = msg_send![string, UTF8String];
                        if !c_str.is_null() {
                            let recognized = std::ffi::CStr::from_ptr(c_str)
                                .to_string_lossy()
                                .into_owned();
                            if !text.is_empty() {
                                text.push(' ');
                            }
                            text.push_str(&recognized);
                        }
                    }
                }
            }

            Ok(text)
        }
    }
}
