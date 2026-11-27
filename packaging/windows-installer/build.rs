use flate2::{Compression, write::GzEncoder};
use std::{env, fs, io::Write, path::PathBuf};

fn main() {
    println!("cargo:rerun-if-env-changed=UXPLAY_STAGE_DIR");
    println!("cargo:rerun-if-env-changed=UXPLAY_VERSION");
    let stage = env::var_os("UXPLAY_STAGE_DIR")
        .map(PathBuf::from)
        .expect("UXPLAY_STAGE_DIR must point to the staged UxPlay runtime");
    let output =
        PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR is set by Cargo")).join("payload.rs");
    let mut files = Vec::new();
    collect_files(&stage, &stage, &mut files);
    assert!(!files.is_empty(), "the staged runtime is empty");
    files.sort_by(|left, right| left.0.cmp(&right.0));
    let payload_directory =
        PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR is set by Cargo")).join("payload");
    fs::create_dir_all(&payload_directory).expect("create compressed payload directory");
    let version =
        env::var("UXPLAY_VERSION").expect("UXPLAY_VERSION must match the app package version");
    let mut source = format!("pub const PRODUCT_VERSION: &str = {version:?};\n");
    source.push_str("pub static PAYLOAD: &[(&str, &[u8])] = &[\n");
    for (index, (relative, absolute)) in files.into_iter().enumerate() {
        println!("cargo:rerun-if-changed={}", absolute.display());
        let compressed_path = payload_directory.join(format!("{index}.gz"));
        let mut encoder = GzEncoder::new(Vec::new(), Compression::best());
        encoder
            .write_all(&fs::read(&absolute).expect("read staged runtime file"))
            .expect("compress staged runtime file");
        fs::write(
            &compressed_path,
            encoder.finish().expect("finish compressed file"),
        )
        .expect("write compressed payload");
        source.push_str(&format!(
            "    ({:?}, include_bytes!({:?})),\n",
            relative.to_string_lossy().replace('\\', "/"),
            compressed_path.to_string_lossy()
        ));
    }
    source.push_str("];\n");
    fs::write(output, source).expect("write embedded payload table");
}

fn collect_files(root: &PathBuf, directory: &PathBuf, files: &mut Vec<(PathBuf, PathBuf)>) {
    println!("cargo:rerun-if-changed={}", directory.display());
    for entry in fs::read_dir(directory).expect("read staged runtime") {
        let path = entry.expect("read staged entry").path();
        if path.is_dir() {
            collect_files(root, &path, files);
        } else if path.is_file() {
            files.push((
                path.strip_prefix(root)
                    .expect("staged file is below root")
                    .into(),
                path,
            ));
        }
    }
}
