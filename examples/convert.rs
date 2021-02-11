use std::{env, fmt, fs, path::Path};

#[derive(Hash, PartialEq, Eq, serde::Deserialize)]
enum Stage {
    Vertex,
    Fragment,
    Compute,
}

#[derive(Hash, PartialEq, Eq, serde::Deserialize)]
struct BindSource {
    stage: Stage,
    group: u32,
    binding: u32,
}

#[derive(serde::Deserialize)]
struct BindTarget {
    #[serde(default)]
    #[cfg_attr(not(feature = "msl-out"), allow(dead_code))]
    buffer: Option<u8>,
    #[serde(default)]
    #[cfg_attr(not(feature = "msl-out"), allow(dead_code))]
    texture: Option<u8>,
    #[serde(default)]
    #[cfg_attr(not(feature = "msl-out"), allow(dead_code))]
    sampler: Option<u8>,
    #[serde(default)]
    #[cfg_attr(not(feature = "msl-out"), allow(dead_code))]
    mutable: bool,
}

#[derive(Default, serde::Deserialize)]
struct Parameters {
    #[serde(default)]
    #[cfg_attr(not(feature = "spv-in"), allow(dead_code))]
    spv_flow_dump_prefix: String,
    #[cfg_attr(not(feature = "spv-out"), allow(dead_code))]
    spv_capabilities: naga::FastHashSet<spirv::Capability>,
    #[cfg_attr(not(feature = "msl-out"), allow(dead_code))]
    mtl_bindings: naga::FastHashMap<BindSource, BindTarget>,
}

trait PrettyResult {
    type Target;
    fn unwrap_pretty(self) -> Self::Target;
}

impl<T, E: fmt::Display> PrettyResult for Result<T, E> {
    type Target = T;
    fn unwrap_pretty(self) -> T {
        match self {
            Result::Ok(value) => value,
            Result::Err(e) => panic!("{}", e),
        }
    }
}

fn main() {
    env_logger::init();

    let args = env::args().collect::<Vec<_>>();
    if args.len() <= 1 {
        println!("Call with <input> <output>");
        return;
    }

    let param_path = std::path::PathBuf::from(&args[1]).with_extension("param.ron");
    let params = match fs::read_to_string(param_path) {
        Ok(string) => ron::de::from_str(&string).unwrap_pretty(),
        Err(_) => {
            let mut param = Parameters::default();
            // very useful to have this by default
            param.spv_capabilities.insert(spirv::Capability::Shader);
            param
        }
    };

    let module = match Path::new(&args[1])
        .extension()
        .expect("Input has no extension?")
        .to_str()
        .unwrap()
    {
        #[cfg(feature = "spv-in")]
        "spv" => {
            let options = naga::front::spv::Options {
                flow_graph_dump_prefix: if params.spv_flow_dump_prefix.is_empty() {
                    None
                } else {
                    Some(params.spv_flow_dump_prefix.into())
                },
            };
            let input = fs::read(&args[1]).unwrap();
            naga::front::spv::parse_u8_slice(&input, &options).unwrap()
        }
        #[cfg(feature = "wgsl-in")]
        "wgsl" => {
            let input = fs::read_to_string(&args[1]).unwrap();
            naga::front::wgsl::parse_str(&input).unwrap_pretty()
        }
        #[cfg(feature = "glsl-in")]
        "vert" => {
            let input = fs::read_to_string(&args[1]).unwrap();
            naga::front::glsl::parse_str(
                &input,
                vec![("main", naga::ShaderStage::Vertex)],
                Default::default(),
            )
            .unwrap_pretty()
        }
        #[cfg(feature = "glsl-in")]
        "frag" => {
            let input = fs::read_to_string(&args[1]).unwrap();
            naga::front::glsl::parse_str(
                &input,
                vec![("main", naga::ShaderStage::Fragment)],
                Default::default(),
            )
            .unwrap_pretty()
        }
        #[cfg(feature = "glsl-in")]
        "comp" => {
            let input = fs::read_to_string(&args[1]).unwrap();
            naga::front::glsl::parse_str(
                &input,
                vec![("main", naga::ShaderStage::Compute)],
                Default::default(),
            )
            .unwrap_pretty()
        }
        #[cfg(feature = "deserialize")]
        "ron" => {
            let mut input = fs::File::open(&args[1]).unwrap();
            ron::de::from_reader(&mut input).unwrap_pretty()
        }
        other => {
            if true {
                // prevent "unreachable_code" warnings
                panic!("Unknown input extension: {}", other);
            }
            naga::Module::generate_empty()
        }
    };

    // validate the IR
    naga::proc::Validator::new()
        .validate(&module)
        .unwrap_pretty();

    if args.len() <= 2 {
        println!("{:#?}", module);
        return;
    }

    match Path::new(&args[2])
        .extension()
        .expect("Output has no extension?")
        .to_str()
        .unwrap()
    {
        #[cfg(feature = "msl-out")]
        "metal" => {
            use naga::back::msl;
            let mut binding_map = msl::BindingMap::default();
            for (key, value) in params.mtl_bindings {
                binding_map.insert(
                    msl::BindSource {
                        stage: match key.stage {
                            Stage::Vertex => naga::ShaderStage::Vertex,
                            Stage::Fragment => naga::ShaderStage::Fragment,
                            Stage::Compute => naga::ShaderStage::Compute,
                        },
                        group: key.group,
                        binding: key.binding,
                    },
                    msl::BindTarget {
                        buffer: value.buffer,
                        texture: value.texture,
                        sampler: value.sampler,
                        mutable: value.mutable,
                    },
                );
            }
            let options = msl::Options {
                lang_version: (1, 0),
                spirv_cross_compatibility: false,
                binding_map,
            };
            let (msl, _) = msl::write_string(&module, &options).unwrap();
            fs::write(&args[2], msl).unwrap();
        }
        #[cfg(feature = "spv-out")]
        "spv" => {
            use naga::back::spv;

            let debug_flag = args.get(3).map_or(spv::WriterFlags::DEBUG, |arg| {
                if arg.parse().unwrap() {
                    spv::WriterFlags::DEBUG
                } else {
                    spv::WriterFlags::NONE
                }
            });

            let spv = spv::write_vec(&module, debug_flag, params.spv_capabilities).unwrap_pretty();

            let bytes = spv
                .iter()
                .fold(Vec::with_capacity(spv.len() * 4), |mut v, w| {
                    v.extend_from_slice(&w.to_le_bytes());
                    v
                });

            fs::write(&args[2], bytes.as_slice()).unwrap();
        }
        #[cfg(feature = "glsl-out")]
        stage @ "vert" | stage @ "frag" | stage @ "comp" => {
            use naga::back::glsl;

            let version = {
                let arg = args.get(3).map_or("es", |p| p.as_str());
                if arg.starts_with("core") {
                    glsl::Version::Desktop(arg[4..].parse().unwrap_or(330))
                } else if arg.starts_with("es") {
                    glsl::Version::Embedded(arg[2..].parse().unwrap_or(310))
                } else {
                    panic!("Unknown profile: {}", arg)
                }
            };
            let name = args.get(4).map_or("main", |p| p.as_str()).to_string();
            let options = glsl::Options {
                version,
                entry_point: (
                    match stage {
                        "vert" => naga::ShaderStage::Vertex,
                        "frag" => naga::ShaderStage::Fragment,
                        "comp" => naga::ShaderStage::Compute,
                        _ => unreachable!(),
                    },
                    name,
                ),
            };

            let file = fs::OpenOptions::new()
                .write(true)
                .truncate(true)
                .create(true)
                .open(&args[2])
                .unwrap();

            let mut writer = glsl::Writer::new(file, &module, &options).unwrap_pretty();

            writer
                .write()
                .map_err(|e| {
                    fs::remove_file(&args[2]).unwrap();
                    e
                })
                .unwrap();
        }
        #[cfg(feature = "serialize")]
        "ron" => {
            let config = ron::ser::PrettyConfig::new()
                .with_enumerate_arrays(true)
                .with_decimal_floats(true);

            let output = ron::ser::to_string_pretty(&module, config).unwrap_pretty();
            fs::write(&args[2], output).unwrap();
        }
        other => {
            let _ = params;
            panic!(
                "Unknown output extension: {}, forgot to enable a feature?",
                other
            );
        }
    }
}
