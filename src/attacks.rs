#[cfg(target_arch = "aarch64")]
pub fn slider_kind() -> &'static str {
    "hq_rbit"
}

#[cfg(target_arch = "wasm32")]
pub fn slider_kind() -> &'static str {
    "black_magic"
}

#[cfg(target_arch = "x86_64")]
pub fn slider_kind() -> &'static str {
    #[cfg(feature = "pext")]
    {
        "pext_or_magic"
    }
    #[cfg(not(feature = "pext"))]
    {
        "black_magic"
    }
}
