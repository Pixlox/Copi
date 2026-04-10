pub const CODE_SEARCH: &[f32; 384] = include!("code_search.rs");
pub const URL_SEARCH: &[f32; 384] = include!("url_search.rs");
pub const IMAGE_SEARCH: &[f32; 384] = include!("image_search.rs");
pub const AUTH_SEARCH: &[f32; 384] = include!("auth_search.rs");
pub const MEETING_SEARCH: &[f32; 384] = include!("meeting_search.rs");
pub const RECIPE_SEARCH: &[f32; 384] = include!("recipe_search.rs");
pub const ERROR_SEARCH: &[f32; 384] = include!("error_search.rs");

pub const ALL: &[(&str, &[f32; 384])] = &[
    ("code", CODE_SEARCH),
    ("url", URL_SEARCH),
    ("image", IMAGE_SEARCH),
    ("auth", AUTH_SEARCH),
    ("meeting", MEETING_SEARCH),
    ("recipe", RECIPE_SEARCH),
    ("error", ERROR_SEARCH),
];
