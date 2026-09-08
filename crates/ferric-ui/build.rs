use slint_build::{CompilerConfiguration, EmbedResourcesKind, compile_with_config};

fn main() {
    // EmbedForSoftwareRenderer bundles the default TTF so the software
    // renderer can rasterize text without a filesystem.
    let config =
        CompilerConfiguration::new().embed_resources(EmbedResourcesKind::EmbedForSoftwareRenderer);
    compile_with_config("ui/main.slint", config).unwrap();
}
