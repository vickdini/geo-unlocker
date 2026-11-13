//! Small line icons (SVG) and the app logo; flag lookup with cached handles.

use iced::widget::svg::Handle;
use std::collections::HashMap;
use std::sync::OnceLock;

macro_rules! icon {
    ($name:ident, $body:expr) => {
        pub fn $name() -> Handle {
            static H: OnceLock<Handle> = OnceLock::new();
            H.get_or_init(|| {
                Handle::from_memory(
                    concat!(
                        r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="#c9c9d4" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round">"##,
                        $body,
                        "</svg>"
                    )
                    .as_bytes(),
                )
            })
            .clone()
        }
    };
}

icon!(power, r#"<path d="M12 4v8"/><path d="M6.6 7.4a7 7 0 1 0 10.8 0"/>"#);
icon!(send, r#"<path d="M4 12 20 4l-5 16-3-7z"/>"#);
icon!(trash, r#"<path d="M4 7h16M9 7V4h6v3M6 7l1 13h10l1-13"/>"#);
icon!(plus, r#"<path d="M12 5v14M5 12h14"/>"#);
icon!(minus, r#"<path d="M5 12h14"/>"#);
icon!(close, r#"<path d="M6 6l12 12M18 6 6 18"/>"#);
icon!(globe, r#"<circle cx="12" cy="12" r="9"/><path d="M3 12h18M12 3c3 3.5 3 14.5 0 18M12 3c-3 3.5-3 14.5 0 18"/>"#);
icon!(lock, r#"<rect x="5" y="11" width="14" height="10" rx="2"/><path d="M8 11V7a4 4 0 0 1 8 0v4"/>"#);
icon!(quit, r#"<path d="M12 3v9"/><path d="M6.3 6.3a8 8 0 1 0 11.4 0"/>"#);

pub fn logo() -> Handle {
    static H: OnceLock<Handle> = OnceLock::new();
    H.get_or_init(|| {
        Handle::from_memory(
            br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24"><defs><linearGradient id="g" x1="0" y1="0" x2="1" y2="1"><stop offset="0" stop-color="#9b85ff"/><stop offset="1" stop-color="#5b3cf0"/></linearGradient></defs><circle cx="12" cy="12" r="11" fill="url(#g)"/><g fill="none" stroke="#fff" stroke-width="1.6"><circle cx="12" cy="12" r="6.5"/><path d="M12 4v16M4 12h16"/><ellipse cx="12" cy="12" rx="3" ry="6.5"/></g></svg>"##
                .as_slice(),
        )
    })
    .clone()
}

/// Flag for an ISO country code (lowercase), if we ship one.
pub fn flag(code: &str) -> Option<Handle> {
    static FLAGS: OnceLock<std::sync::Mutex<HashMap<String, Handle>>> = OnceLock::new();
    let map = FLAGS.get_or_init(Default::default);
    let mut m = map.lock().unwrap();
    if let Some(h) = m.get(code) {
        return Some(h.clone());
    }
    let h = Handle::from_memory(super::flags::svg(code)?);
    m.insert(code.to_string(), h.clone());
    Some(h)
}

/// Country name in English from an ISO code.
pub fn country_name(code: &str) -> String {
    let c = code.to_ascii_lowercase();
    let name = match c.as_str() {
        "" => "Any country",
        "ae" => "United Arab Emirates", "al" => "Albania", "am" => "Armenia", "ar" => "Argentina", "at" => "Austria",
        "au" => "Australia", "az" => "Azerbaijan", "ba" => "Bosnia and Herzegovina", "bd" => "Bangladesh", "be" => "Belgium",
        "bg" => "Bulgaria", "bo" => "Bolivia", "br" => "Brazil", "by" => "Belarus", "ca" => "Canada", "ch" => "Switzerland",
        "cl" => "Chile", "cn" => "China", "co" => "Colombia", "cr" => "Costa Rica", "cy" => "Cyprus", "cz" => "Czechia",
        "de" => "Germany", "dk" => "Denmark", "do" => "Dominican Republic", "dz" => "Algeria", "ec" => "Ecuador",
        "ee" => "Estonia", "eg" => "Egypt", "es" => "Spain", "fi" => "Finland", "fr" => "France", "gb" => "United Kingdom",
        "ge" => "Georgia", "gr" => "Greece", "gt" => "Guatemala", "hk" => "Hong Kong", "hr" => "Croatia", "hu" => "Hungary",
        "id" => "Indonesia", "ie" => "Ireland", "il" => "Israel", "in" => "India", "iq" => "Iraq", "ir" => "Iran",
        "is" => "Iceland", "it" => "Italy", "jp" => "Japan", "ke" => "Kenya", "kr" => "South Korea", "kz" => "Kazakhstan",
        "lt" => "Lithuania", "lu" => "Luxembourg", "lv" => "Latvia", "ma" => "Morocco", "md" => "Moldova", "me" => "Montenegro",
        "mk" => "North Macedonia", "mt" => "Malta", "mx" => "Mexico", "my" => "Malaysia", "ng" => "Nigeria", "nl" => "Netherlands",
        "no" => "Norway", "nz" => "New Zealand", "pa" => "Panama", "pe" => "Peru", "ph" => "Philippines", "pk" => "Pakistan",
        "pl" => "Poland", "pt" => "Portugal", "py" => "Paraguay", "qa" => "Qatar", "ro" => "Romania", "rs" => "Serbia",
        "ru" => "Russia", "sa" => "Saudi Arabia", "se" => "Sweden", "sg" => "Singapore", "si" => "Slovenia", "sk" => "Slovakia",
        "sc" => "Seychelles", "th" => "Thailand", "tn" => "Tunisia", "tr" => "Türkiye", "tw" => "Taiwan", "ua" => "Ukraine",
        "us" => "United States", "uy" => "Uruguay", "uz" => "Uzbekistan", "ve" => "Venezuela", "vn" => "Vietnam",
        "za" => "South Africa", "li" => "Liechtenstein", "mc" => "Monaco", "ad" => "Andorra", "bh" => "Bahrain", "mu" => "Mauritius",
        "kh" => "Cambodia", "lk" => "Sri Lanka", "np" => "Nepal", "mn" => "Mongolia", "kg" => "Kyrgyzstan", "tj" => "Tajikistan",
        "jo" => "Jordan", "lb" => "Lebanon", "kw" => "Kuwait", "om" => "Oman", "cu" => "Cuba", "jm" => "Jamaica",
        "ni" => "Nicaragua", "hn" => "Honduras", "sv" => "El Salvador", "gh" => "Ghana", "tz" => "Tanzania", "ug" => "Uganda",
        "et" => "Ethiopia", "ao" => "Angola", "mz" => "Mozambique", "zw" => "Zimbabwe", "zm" => "Zambia", "na" => "Namibia",
        "bw" => "Botswana", "cm" => "Cameroon", "ci" => "Côte d'Ivoire", "sn" => "Senegal", "mg" => "Madagascar",
        "af" => "Afghanistan", "mm" => "Myanmar", "la" => "Laos", "pg" => "Papua New Guinea", "fj" => "Fiji",
        _ => return c.to_uppercase(),
    };
    name.to_string()
}
