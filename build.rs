//! Baut den UxPlay-Kern (third_party/uxplay/lib, ohne dnssd.c) per `cc`, verlinkt FFmpeg,
//! SDL3, OpenSSL, libplist und die Windows-Systembibliotheken und erzeugt per bindgen die
//! Bindings für UxPlay (`src/ffi/wrapper_uxplay.h`) und FFmpeg (`src/ffi/wrapper_ffmpeg.h`).
//! Gebaut wird in der MSYS2-UCRT64-Umgebung (GCC + pkg-config + libclang aus /ucrt64).

use std::env;
use std::fs;
use std::path::{Path, PathBuf};

const UXPLAY_LIB: &str = "third_party/uxplay/lib";

/// Defines aus v1-`CMakeLists.txt` (uxplay_core, PRIVATE + PUBLIC).
const UXPLAY_DEFINES: &[(&str, Option<&str>)] = &[
    ("STANDALONE", None),
    ("__STDC_CONSTANT_MACROS", None),
    ("__STDC_LIMIT_MACROS", None),
    ("TARGET_POSIX", None),
    ("_REENTRANT", None),
    ("_LARGEFILE64_SOURCE", None),
    ("_FILE_OFFSET_BITS", Some("64")),
    ("NOHOLD", None),
    ("PLIST_210", None),
    ("PLIST_230", None),
    ("OPENSSL_API_COMPAT", Some("0x10101000L")),
];

/// pkg-config-Pakete mit Mindestversion (OpenSSL und libplist wie in v1-`CMakeLists.txt`).
const PKG_CONFIG_LIBS: &[(&str, Option<&str>)] = &[
    ("libavcodec", None),
    ("libavformat", None),
    ("libavutil", None),
    ("libswscale", None),
    ("libswresample", None),
    ("sdl3", None),
    ("openssl", Some("1.1.1")),
    ("libplist-2.0", Some("2.3.0")),
];

const SYSTEM_LIBS: &[&str] = &[
    "ws2_32",
    "iphlpapi",
    "dnsapi",
    "wsock32",
    "winmm",
    "ole32",
    "oleaut32",
    "uuid",
    "shell32",
    "d3d11",
    "dxgi",
    "d3dcompiler",
    // CryptGenRandom in srp.c; unter CMake implizit über die GCC-Standardbibliotheken,
    // rustc linkt mit -nodefaultlibs.
    "advapi32",
];

const FFMPEG_ALLOWLIST: &str = "av.*|AV.*|sws_.*|swr_.*|SWS_.*|FF_.*";

const SWSCALE_SWRESAMPLE_TYPES: &str = "Sws.*|Swr.*";

/// Die von `AVD3D11VADeviceContext`/`AVD3D11VAFramesContext` benutzten D3D11-Schnittstellen.
const D3D11_OPAQUE: &str = "ID3D11(Device|DeviceContext|VideoDevice|VideoContext|Texture2D)";

fn main() {
    let out_dir = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR fehlt"));
    let uxplay_lib = Path::new(UXPLAY_LIB);

    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=src/ffi/wrapper_uxplay.h");
    println!("cargo:rerun-if-changed=src/ffi/wrapper_ffmpeg.h");
    println!("cargo:rerun-if-changed=third_party");

    // Der statische UxPlay-Kern wird ins rlib gebündelt. Seine Verweise auf OpenSSL, libplist
    // und die Rust-definierten `dnssd_*` löst der Linker beim Endlink auf; die
    // pkg-config-Bibliotheken (`cargo:rustc-link-lib`) stehen dort hinter dem rlib.
    let include_paths = link_pkg_config_libs();
    build_uxplay_core(uxplay_lib, &include_paths);
    for lib in SYSTEM_LIBS {
        println!("cargo:rustc-link-lib={lib}");
    }

    use_msys2_libclang();
    generate_uxplay_bindings(uxplay_lib, &include_paths, &out_dir);
    generate_ffmpeg_bindings(&include_paths, &out_dir);
}

/// Kompiliert alle `.c` unter `third_party/uxplay/lib` außer `dnssd.c` (ersetzt durch eine
/// eigene Implementierung mit derselben API).
fn build_uxplay_core(uxplay_lib: &Path, include_paths: &[PathBuf]) {
    let mut sources = Vec::new();
    collect_c_sources(uxplay_lib, &mut sources);
    sources.retain(|p| p != &uxplay_lib.join("dnssd.c"));
    sources.sort();
    assert!(!sources.is_empty(), "keine UxPlay-Quellen in {UXPLAY_LIB}");

    let mut build = cc::Build::new();
    build
        .files(&sources)
        .include(uxplay_lib)
        .include(uxplay_lib.join("llhttp"))
        .include(uxplay_lib.join("playfair"))
        .includes(include_paths)
        .std("gnu11")
        .warnings(false)
        .flag("-w");
    for (name, value) in UXPLAY_DEFINES {
        build.define(name, *value);
    }
    build.compile("uxplay_core");
}

fn collect_c_sources(dir: &Path, out: &mut Vec<PathBuf>) {
    let entries = fs::read_dir(dir).unwrap_or_else(|e| panic!("{}: {e}", dir.display()));
    for entry in entries {
        let path = entry.expect("Verzeichniseintrag").path();
        if path.is_dir() {
            collect_c_sources(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "c") {
            out.push(path);
        }
    }
}

/// Verlinkt per pkg-config und gibt die Include-Pfade zurück.
fn link_pkg_config_libs() -> Vec<PathBuf> {
    let mut includes = Vec::new();
    for (name, min_version) in PKG_CONFIG_LIBS {
        let mut config = pkg_config::Config::new();
        if let Some(version) = min_version {
            config.atleast_version(version);
        }
        let lib = config
            .probe(name)
            .unwrap_or_else(|e| panic!("pkg-config {name}: {e}"));
        includes.extend(lib.include_paths);
    }
    includes.sort();
    includes.dedup();
    includes
}

/// clang-sys fände sonst womöglich ein fremdes libclang (z. B. LLVM aus Visual Studio mit
/// MSVC-Ziel), das die MinGW-Header nicht parsen kann. Ohne gesetztes `LIBCLANG_PATH` wird
/// das libclang derselben MSYS2-Umgebung verwendet, aus der auch FFmpeg kommt
/// (`<prefix>/bin`, z. B. `C:/msys64/ucrt64/bin`).
fn use_msys2_libclang() {
    println!("cargo:rerun-if-env-changed=LIBCLANG_PATH");
    if env::var_os("LIBCLANG_PATH").is_some() {
        return;
    }
    let prefix = pkg_config::get_variable("libavcodec", "prefix")
        .unwrap_or_else(|e| panic!("pkg-config prefix libavcodec: {e}"));
    let bin = Path::new(&prefix).join("bin");
    // SAFETY: Das Build-Skript ist an dieser Stelle einfädig; clang-sys liest die Variable
    // erst beim ersten bindgen-Aufruf danach.
    unsafe { env::set_var("LIBCLANG_PATH", bin) };
}

fn clang_include_args(include_paths: &[PathBuf]) -> Vec<String> {
    include_paths
        .iter()
        .map(|p| format!("-I{}", p.display()))
        .collect()
}

fn base_builder(header: &str, include_paths: &[PathBuf]) -> bindgen::Builder {
    bindgen::Builder::default()
        .header(header)
        .clang_args(clang_include_args(include_paths))
        .rust_edition(bindgen::RustEdition::Edition2024)
        .prepend_enum_name(false)
        .parse_callbacks(Box::new(bindgen::CargoCallbacks::new()))
}

fn generate_uxplay_bindings(uxplay_lib: &Path, include_paths: &[PathBuf], out_dir: &Path) {
    let mut builder = base_builder("src/ffi/wrapper_uxplay.h", include_paths)
        .clang_arg(format!("-I{}", uxplay_lib.display()))
        .allowlist_file(r".*third_party[/\\]uxplay[/\\]lib[/\\].*");
    for (name, value) in UXPLAY_DEFINES {
        builder = builder.clang_arg(match value {
            Some(v) => format!("-D{name}={v}"),
            None => format!("-D{name}"),
        });
    }
    builder
        .generate()
        .expect("bindgen UxPlay")
        .write_to_file(out_dir.join("uxplay.rs"))
        .expect("uxplay.rs schreiben");
}

fn generate_ffmpeg_bindings(include_paths: &[PathBuf], out_dir: &Path) {
    base_builder("src/ffi/wrapper_ffmpeg.h", include_paths)
        .allowlist_function(FFMPEG_ALLOWLIST)
        .allowlist_type(FFMPEG_ALLOWLIST)
        .allowlist_var(FFMPEG_ALLOWLIST)
        // bindgen übernimmt Varianten benannter Enums nicht über allowlist_var; ohne diese
        // Typen fehlten z. B. SWS_BILINEAR (enum SwsFlags) und SWR_DITHER_* (enum SwrDitherType).
        .allowlist_type(SWSCALE_SWRESAMPLE_TYPES)
        // D3D11-Schnittstellen aus hwcontext_d3d11va.h nur als opake Typen (Zugriff per Zeiger).
        .allowlist_type(D3D11_OPAQUE)
        .opaque_type(D3D11_OPAQUE)
        .generate()
        .expect("bindgen FFmpeg")
        .write_to_file(out_dir.join("ffmpeg.rs"))
        .expect("ffmpeg.rs schreiben");
}
