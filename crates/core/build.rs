fn main() {
    if version_check::is_min_version("1.70.0").unwrap_or(false) {
        println!("cargo:rustc-cfg=has_arc_into_inner");
    }
}
