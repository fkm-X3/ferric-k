use std::path::PathBuf;

use slint_build::{CompilerConfiguration, EmbedResourcesKind, compile_with_config};

fn main() {
    // EmbedForSoftwareRenderer bundles the TTF fonts as pre-rasterized glyph
    // data so the software renderer can draw text without a filesystem. The
    // bundled fonts (DejaVu Sans) live in the workspace-level `fonts/` dir and
    // are referenced by bare name from the `.slint` via this include path.
    let fonts_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fonts");
    let config =
        CompilerConfiguration::new().embed_resources(EmbedResourcesKind::EmbedForSoftwareRenderer);
    let config = config.with_include_paths(vec![fonts_dir]);
    // CompilerConfiguration is not re-cloned between runs; each entry point
    // gets its own `compile_with_config` so `include_modules!` sees both.
    compile_with_config("ui/main.slint", config.clone()).unwrap();
    compile_with_config("ui/monitor.slint", config).unwrap();
}
