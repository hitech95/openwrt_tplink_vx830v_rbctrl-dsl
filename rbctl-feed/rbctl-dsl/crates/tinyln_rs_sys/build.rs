use std::env;
use std::path::PathBuf;

fn main() {
    let libnl_dir = env::var("LIBNL_TINY_DIR").unwrap_or_else(|_| "/usr".to_string());
    let include_dir = format!("{libnl_dir}/include/libnl-tiny");

    let bindings = bindgen::Builder::default()
        .header("wrapper.h")
        .clang_arg(format!("-I{include_dir}"))
        .clang_arg("-D_GNU_SOURCE")
        .clang_arg("-Wno-incompatible-function-pointer-types")
        .ctypes_prefix("libc")
        .generate_inline_functions(true)
        // libnl-tiny functions
        .allowlist_function("nl_.*")
        .allowlist_function("nlmsg_.*")
        .allowlist_function("nla_.*")
        .allowlist_function("unl_.*")
        .allowlist_function("genlmsg_.*")
        // libnl-tiny types
        .allowlist_type("nl_.*")
        .allowlist_var("NL_.*")
        .allowlist_var("NLA_.*")
        .allowlist_var("NLM_.*")
        // kernel rtnetlink
        .allowlist_var("RTM_.*")
        .allowlist_var("RTA_.*")
        .allowlist_var("RTN_.*")
        .allowlist_var("RTPROT_.*")
        .allowlist_var("RT_SCOPE_.*")
        .allowlist_var("IFLA_.*")
        .allowlist_var("IFA_.*")
        .allowlist_var("IFF_.*")
        .allowlist_var("IFNAMSIZ")
        .allowlist_var("GENL_.*")
        .allowlist_type("ifinfomsg")
        .allowlist_type("ifaddrmsg")
        .allowlist_type("rtmsg")
        .allowlist_type("genlmsghdr")
        // misc
        .default_enum_style(bindgen::EnumVariation::Consts)
        .prepend_enum_name(false)
        .derive_default(true)
        .layout_tests(false)
        .generate()
        .expect("unable to generate tinyln-rs bindings");

    let out_path = PathBuf::from(env::var("OUT_DIR").unwrap());
    bindings
        .write_to_file(out_path.join("bindings.rs"))
        .expect("couldn't write bindings.rs");

    println!("cargo:rustc-link-search=native={libnl_dir}/lib");
    println!("cargo:rustc-link-lib=dylib=nl-tiny");
    println!("cargo:rerun-if-changed=wrapper.h");
}
