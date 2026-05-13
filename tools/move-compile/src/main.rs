use move_compiler::{
    editions::Edition,
    shared::{NumericalAddress, PackageConfig, PackagePaths},
    Compiler, Flags,
};
use move_core_types::account_address::AccountAddress;
use move_symbol_pool::Symbol;
use std::collections::BTreeMap;
use std::path::PathBuf;

fn main() {
    let args: Vec<String> = std::env::args().collect();

    let mut sources = Vec::new();
    let mut out_dir = PathBuf::from(".");
    let mut named_addrs: BTreeMap<Symbol, NumericalAddress> = BTreeMap::new();
    let mut dep_paths = Vec::new();
    let mut parsing_deps = false;
    let mut i = 1;
    while i < args.len() {
        if args[i] == "--" {
            parsing_deps = true;
            i += 1;
            continue;
        }
        if parsing_deps {
            dep_paths.push(args[i].clone());
            i += 1;
            continue;
        }
        if args[i] == "--out" {
            i += 1;
            if i < args.len() { out_dir = PathBuf::from(&args[i]); }
            i += 1;
            continue;
        }
        if args[i] == "--addr" {
            i += 1;
            if i < args.len() {
                let parts: Vec<&str> = args[i].splitn(2, '=').collect();
                if parts.len() == 2 {
                    let hex = format!("0x{}", parts[1].trim_start_matches("0x"));
                    let addr = NumericalAddress::new(
                        AccountAddress::from_hex_literal(&hex)
                            .expect("Invalid hex address")
                            .into_bytes(),
                        move_compiler::shared::NumberFormat::Hex,
                    );
                    named_addrs.insert(Symbol::from(parts[0]), addr);
                }
            }
            i += 1;
            continue;
        }
        sources.push(args[i].clone());
        i += 1;
    }

    if sources.is_empty() {
        eprintln!("Usage: move-compile <sources...> --out <dir> [--addr name=hex ...] [-- <dep_dirs...>]");
        std::process::exit(1);
    }

    std::fs::create_dir_all(&out_dir).expect("Failed to create output directory");

    let mut source_files: Vec<Symbol> = Vec::new();
    for src in &sources {
        let path = PathBuf::from(src);
        if path.is_dir() {
            for entry in std::fs::read_dir(&path).expect("Failed to read dir") {
                let entry = entry.expect("Failed to read entry");
                let p = entry.path();
                if p.extension().map_or(false, |e| e == "move") {
                    source_files.push(Symbol::from(p.to_string_lossy().as_ref()));
                }
            }
        } else {
            source_files.push(Symbol::from(src.as_str()));
        }
    }

    let mut dep_files: Vec<Symbol> = Vec::new();
    for dep_dir in &dep_paths {
        let path = PathBuf::from(dep_dir);
        if path.is_dir() {
            for entry in std::fs::read_dir(&path).expect("Failed to read dep dir") {
                let entry = entry.expect("Failed to read entry");
                let p = entry.path();
                if p.extension().map_or(false, |e| e == "move") {
                    dep_files.push(Symbol::from(p.to_string_lossy().as_ref()));
                }
            }
        }
    }

    let pkg_name = Symbol::from("target");
    let pkg_config = PackageConfig {
        edition: Edition::LEGACY,
        ..PackageConfig::default()
    };

    let targets = vec![PackagePaths {
        name: Some((pkg_name, pkg_config.clone())),
        paths: source_files,
        named_address_map: named_addrs.clone(),
    }];

    let deps = if dep_files.is_empty() {
        vec![]
    } else {
        let dep_name = Symbol::from("deps");
        let dep_config = PackageConfig {
            is_dependency: true,
            edition: Edition::LEGACY,
            ..PackageConfig::default()
        };
        vec![PackagePaths {
            name: Some((dep_name, dep_config)),
            paths: dep_files,
            named_address_map: named_addrs.clone(),
        }]
    };

    let compiler = Compiler::from_package_paths(None, targets, deps)
        .expect("Failed to create compiler")
        .set_flags(Flags::empty());

    let (_, compiled_units) = compiler
        .build_and_report()
        .expect("Compilation failed");

    let mut count = 0;
    for unit in &compiled_units {
        let name = unit.named_module.name.as_str();
        let mut bytes = Vec::new();
        unit.named_module.module.serialize_with_version(
            unit.named_module.module.version, &mut bytes,
        ).expect("Serialization failed");
        let out_path = out_dir.join(format!("{}.mv", name));
        std::fs::write(&out_path, &bytes).expect("Failed to write");
        println!("Compiled: {} -> {} ({} bytes)", name, out_path.display(), bytes.len());
        count += 1;
    }
    println!("Done: {} modules compiled", count);
}
