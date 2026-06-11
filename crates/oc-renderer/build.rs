//! Compiles WGSL shaders to SPIR-V at build time with naga (pure Rust, no
//! system Vulkan SDK needed). Each `shaders/*.wgsl` becomes `OUT_DIR/<name>.spv`
//! containing all of the file's entry points.

use std::path::Path;

fn main() {
    println!("cargo::rerun-if-changed=shaders");
    let out_dir = std::env::var("OUT_DIR").unwrap();

    for entry in std::fs::read_dir("shaders").expect("shaders/ dir") {
        let path = entry.unwrap().path();
        if path.extension().and_then(|e| e.to_str()) != Some("wgsl") {
            continue;
        }
        let name = path.file_stem().unwrap().to_str().unwrap();
        let spv = compile(&path).unwrap_or_else(|e| panic!("compiling {name}.wgsl: {e}"));

        let mut bytes = Vec::with_capacity(spv.len() * 4);
        for word in spv {
            bytes.extend_from_slice(&word.to_le_bytes());
        }
        std::fs::write(Path::new(&out_dir).join(format!("{name}.spv")), bytes).unwrap();
    }
}

fn compile(path: &Path) -> Result<Vec<u32>, String> {
    let source = std::fs::read_to_string(path).map_err(|e| e.to_string())?;

    let module = naga::front::wgsl::parse_str(&source)
        .map_err(|e| e.emit_to_string(&source))?;

    let info = naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::all(),
    )
    .validate(&module)
    .map_err(|e| format!("{e:?}"))?;

    let options = naga::back::spv::Options {
        lang_version: (1, 3),
        ..Default::default()
    };
    naga::back::spv::write_vec(&module, &info, &options, None).map_err(|e| e.to_string())
}
