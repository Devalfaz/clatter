use std::path::PathBuf;

#[path = "src/icon.rs"]
#[allow(dead_code)]
mod icon;

fn main() {
    println!("cargo:rerun-if-changed=src/icon.rs");
    println!("cargo:rerun-if-changed=app.manifest");

    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap());

    // Application icon (.ico with several sizes), rendered from code.
    let ico_path = out.join("clatter.ico");
    let mut dir = ico::IconDir::new(ico::ResourceType::Icon);
    for size in [16u32, 20, 24, 32, 40, 48, 64, 128, 256] {
        let rgba = icon::render(size, icon::IconStyle::Color);
        let image = ico::IconImage::from_rgba_data(size, size, rgba);
        dir.add_entry(ico::IconDirEntry::encode(&image).unwrap());
    }
    dir.write(std::fs::File::create(&ico_path).unwrap()).unwrap();

    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap()).join("app.manifest");
        let esc = |p: &std::path::Path| p.display().to_string().replace('\\', "/");
        let rc = format!(
            "1 ICON \"{}\"\n1 24 \"{}\"\n",
            esc(&ico_path),
            esc(&manifest)
        );
        let rc_path = out.join("clatter.rc");
        std::fs::write(&rc_path, rc).unwrap();
        embed_resource::compile(&rc_path, embed_resource::NONE).manifest_required().unwrap();
    }

    let config = slint_build::CompilerConfiguration::new().with_style("fluent".into());
    slint_build::compile_with_config("ui/app.slint", config).unwrap();
}
