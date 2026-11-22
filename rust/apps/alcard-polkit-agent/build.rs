fn main() {
    // Bind only the stable session/identity C API. Keep authentication in the
    // distribution's libpolkit-agent and its PAM helper.
    for library in ["polkit-agent-1", "polkit-gobject-1"] {
        pkg_config::Config::new()
            .probe(library)
            .unwrap_or_else(|error| panic!("{library} development files are required: {error}"));
    }
}
