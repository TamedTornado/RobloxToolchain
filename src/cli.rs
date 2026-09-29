use clap::{Parser, Subcommand};
use roblox_toolchain::{Result, asset_commands};
use serde_json::{Value, json};
use std::{path::PathBuf, process::ExitCode};

#[derive(Parser)]
#[command(
    name = "roblox",
    about = "CLI-first Roblox project automation",
    version
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Upload, link and publish verified outputs separately from offline builds.
    Deploy {
        #[command(subcommand)]
        command: Deploy,
    },
    /// Verify a local bundle's declared runtime hashes and native references.
    VerifyBundle { directory: PathBuf },
    /// Compile Luau locally without executing source.
    Compile {
        source: PathBuf,
        #[arg(long)]
        config: PathBuf,
        #[arg(long)]
        output: PathBuf,
    },
    /// Assemble native scene artifacts offline from explicitly typed JSON.
    Build {
        #[command(subcommand)]
        command: Build,
    },
    /// Convert assets locally. No catalog, Studio, credentials or network.
    Convert {
        #[command(subcommand)]
        command: Convert,
    },
    /// Report implemented capabilities without requiring Studio or a catalog.
    Capabilities,
    /// Manage logical assets directly in an offline build plan.
    Assets {
        #[arg(long, global = true)]
        plan: Option<PathBuf>,
        #[command(subcommand)]
        command: asset_commands::Command,
    },
}

#[derive(Subcommand)]
enum Deploy {
    /// Upload selected prebuilt dependencies, wait for moderation and link the place.
    Upload(CloudArgs),
    /// Explicitly retry a definitively failed upload operation on the next run.
    RetryUpload {
        #[command(flatten)]
        args: CloudArgs,
        #[arg(long)]
        receipt: String,
    },
    /// Upload/link as needed and publish to the explicitly configured destination.
    Publish(CloudArgs),
    /// Read durable receipts. Does not use credentials or contact Roblox.
    Status {
        #[arg(long)]
        state: PathBuf,
    },
    /// Associate an explicitly identified operation with an ambiguous upload receipt.
    ReconcileOperation {
        #[command(flatten)]
        args: CloudArgs,
        #[arg(long)]
        receipt: String,
        #[arg(long)]
        operation_id: String,
    },
    /// Verify an existing place version against an ambiguous publication's hash.
    ReconcilePublication {
        #[command(flatten)]
        args: CloudArgs,
        #[arg(long)]
        sha256: String,
        #[arg(long)]
        version: u64,
    },
    /// Link typed native scene references to caller-supplied remote IDs. No upload.
    LinkScene {
        directory: PathBuf,
        #[arg(long)]
        scene: String,
        #[arg(long)]
        mapping: PathBuf,
        #[arg(long)]
        output: PathBuf,
    },
}

#[derive(clap::Args)]
struct CloudArgs {
    directory: PathBuf,
    #[arg(long)]
    config: PathBuf,
    #[arg(long)]
    state: PathBuf,
}

#[derive(Subcommand)]
enum Build {
    /// Mux preconverted local VP9 video and Vorbis/Opus audio into WebM.
    Media {
        source: PathBuf,
        #[arg(long)]
        output: PathBuf,
    },
    /// Convert assets and assemble locally linked scenes from a JSON plan.
    Bundle {
        source: PathBuf,
        #[arg(long)]
        output: PathBuf,
    },
    Scene {
        source: PathBuf,
        #[arg(long)]
        output: PathBuf,
    },
}

#[derive(Subcommand)]
enum Convert {
    /// Convert configured sparse voxels or a scalar heightmap to SmoothGrid bytes.
    Terrain {
        source: PathBuf,
        #[arg(long)]
        output: PathBuf,
    },
    /// Transcode a local video/audio source to combined VP9/Vorbis WebM.
    Media {
        source: PathBuf,
        #[arg(long)]
        config: PathBuf,
        #[arg(long)]
        output: PathBuf,
    },
    /// Convert a supported local silent video to VP9/WebM in process.
    Video {
        source: PathBuf,
        #[arg(long)]
        config: PathBuf,
        #[arg(long)]
        output: PathBuf,
    },
    /// Extract a core glTF PBR material and bake factors into native texture maps.
    MaterialGltf {
        source: PathBuf,
        #[arg(long)]
        config: PathBuf,
        #[arg(long)]
        output: PathBuf,
    },
    /// Build a native SurfaceAppearance and local texture maps from material JSON.
    Material {
        source: PathBuf,
        #[arg(long)]
        output: PathBuf,
    },
    /// Bake FBX motion locally using explicit sampling configuration.
    AnimationFbx {
        source: PathBuf,
        #[arg(long)]
        config: PathBuf,
        #[arg(long)]
        output: PathBuf,
    },
    /// Import rigid-bind GLB/glTF or linear FBX skins as native v4.01 meshes.
    Skin {
        source: PathBuf,
        #[arg(long)]
        config: PathBuf,
        #[arg(long)]
        output: PathBuf,
    },
    /// Convert rigid LINEAR glTF/GLB skeletal animation locally.
    AnimationGltf {
        source: PathBuf,
        #[arg(long)]
        config: PathBuf,
        #[arg(long)]
        output: PathBuf,
    },
    /// Encode canonical rest-relative animation JSON as a native KeyframeSequence.
    Animation {
        source: PathBuf,
        #[arg(long)]
        output: PathBuf,
    },
    /// Decode audio and encode Ogg Vorbis locally, preserving sample rate/channels.
    Audio {
        source: PathBuf,
        #[arg(long)]
        config: PathBuf,
        #[arg(long)]
        output: PathBuf,
    },
    /// Convert images and PBR maps locally to configured PNG or DDS artifacts.
    Texture {
        source: PathBuf,
        #[arg(long)]
        config: PathBuf,
        #[arg(long)]
        output: PathBuf,
    },
    /// Encode static GLB/glTF, FBX or OBJ geometry as native Roblox meshes.
    Mesh {
        source: PathBuf,
        #[arg(long)]
        config: PathBuf,
        #[arg(long)]
        output: PathBuf,
        /// Optional local collision recipe; payload engine acceptance is unverified.
        #[arg(long)]
        collision_config: Option<PathBuf>,
    },
}

fn execute(cli: Cli) -> Result<Value> {
    if let Command::Convert {
        command: Convert::Terrain { source, output },
    } = &cli.command
    {
        let result = roblox_toolchain::terrain::convert(source, output)?;
        return Ok(json!({"ok":true,"scope":"offlineConversion","result":result}));
    }
    if let Command::Convert {
        command:
            Convert::AnimationFbx {
                source,
                config,
                output,
            },
    } = &cli.command
    {
        let config = serde_json::from_slice(&std::fs::read(config)?)?;
        return Ok(
            json!({"ok":true,"scope":"offlineConversion","result":roblox_toolchain::animation_fbx::convert(source,output,config)?}),
        );
    }
    if let Command::Convert {
        command:
            Convert::Skin {
                source,
                config,
                output,
            },
    } = &cli.command
    {
        let config = serde_json::from_slice(&std::fs::read(config)?)?;
        return Ok(
            json!({"ok":true,"scope":"offlineConversion","result":roblox_toolchain::skin_import::convert(source,output,&config)?}),
        );
    }
    if let Command::Convert {
        command:
            Convert::AnimationGltf {
                source,
                config,
                output,
            },
    } = &cli.command
    {
        let config = serde_json::from_slice(&std::fs::read(config)?)?;
        return Ok(
            json!({"ok":true,"scope":"offlineConversion","result":roblox_toolchain::animation_gltf::convert(source,output,config)?}),
        );
    }
    if let Command::Convert {
        command: Convert::Animation { source, output },
    } = &cli.command
    {
        return Ok(
            json!({"ok":true,"scope":"offlineConversion","result":roblox_toolchain::animation::convert(source,output)?}),
        );
    }
    if let Command::Build {
        command: Build::Media { source, output },
    } = &cli.command
    {
        return Ok(
            json!({"ok":true,"scope":"offlineMediaBuild","result":roblox_toolchain::media_mux::build(source,output)?}),
        );
    }
    if let Command::Build {
        command: Build::Bundle { source, output },
    } = &cli.command
    {
        return Ok(
            json!({"ok":true,"scope":"offlineBundleBuild","result":roblox_toolchain::bundle::build(source,output)?}),
        );
    }
    if let Command::Convert {
        command:
            Convert::Audio {
                source,
                config,
                output,
            },
    } = &cli.command
    {
        let config = serde_json::from_slice(&std::fs::read(config)?)?;
        return Ok(
            json!({"ok":true,"scope":"offlineConversion","result":roblox_toolchain::audio::convert(source,output,&config)?}),
        );
    }
    if let Command::Compile {
        source,
        config,
        output,
    } = &cli.command
    {
        let config = serde_json::from_slice(&std::fs::read(config)?)?;
        return Ok(
            json!({"ok":true,"scope":"offlineCompilation","result":roblox_toolchain::scripts::compile_file(source,output,&config)?}),
        );
    }
    if let Command::Build {
        command: Build::Scene { source, output },
    } = &cli.command
    {
        return Ok(
            json!({"ok":true,"scope":"offlineSceneBuild","result":roblox_toolchain::scene::build(source,output)?}),
        );
    }
    if let Command::Convert {
        command:
            Convert::Texture {
                source,
                config,
                output,
            },
    } = &cli.command
    {
        let config = serde_json::from_slice(&std::fs::read(config)?)?;
        let manifest = roblox_toolchain::texture::convert(source, output, &config)?;
        return Ok(json!({"ok":true,"scope":"offlineConversion","result":manifest}));
    }
    if let Command::Convert {
        command: Convert::Material { source, output },
    } = &cli.command
    {
        let manifest = roblox_toolchain::material::convert(source, output)?;
        return Ok(json!({"ok":true,"scope":"offlineConversion","result":manifest}));
    }
    if let Command::Convert {
        command:
            Convert::Mesh {
                source,
                config,
                output,
                collision_config,
            },
    } = &cli.command
    {
        let config = serde_json::from_slice(&std::fs::read(config)?)?;
        let collision = collision_config
            .as_ref()
            .map(|path| -> Result<_> {
                Ok(
                    serde_json::from_slice::<roblox_toolchain::collision::Recipe>(&std::fs::read(
                        path,
                    )?)?,
                )
            })
            .transpose()?;
        let manifest = roblox_toolchain::convert::convert_with_collision(
            source,
            output,
            &config,
            collision.as_ref(),
        )?;
        return Ok(json!({"ok":true,"scope":"offlineConversion","result":manifest}));
    }
    if let Command::Convert {
        command:
            Convert::MaterialGltf {
                source,
                config,
                output,
            },
    } = &cli.command
    {
        let config = serde_json::from_slice(&std::fs::read(config)?)?;
        let manifest = roblox_toolchain::material_gltf::convert(source, output, &config)?;
        return Ok(json!({"ok":true,"scope":"offlineConversion","result":manifest}));
    }
    if let Command::Convert {
        command:
            Convert::Video {
                source,
                config,
                output,
            },
    } = &cli.command
    {
        let config = serde_json::from_slice(&std::fs::read(config)?)?;
        let manifest = roblox_toolchain::video::convert(source, output, &config)?;
        return Ok(json!({"ok":true,"scope":"offlineConversion","result":manifest}));
    }
    if let Command::Convert {
        command:
            Convert::Media {
                source,
                config,
                output,
            },
    } = &cli.command
    {
        let config = serde_json::from_slice(&std::fs::read(config)?)?;
        let manifest = roblox_toolchain::media_transcode::convert(source, output, &config)?;
        return Ok(json!({"ok":true,"scope":"offlineConversion","result":manifest}));
    }
    if let Command::VerifyBundle { directory } = &cli.command {
        let result = roblox_toolchain::bundle_verify::verify(directory)?;
        return Ok(json!({"ok":true,"scope":"offlineBundleVerification","result":result}));
    }
    if let Command::Deploy { command } = &cli.command {
        use roblox_toolchain::cloud_deploy;
        let (scope, result) = match command {
            Deploy::RetryUpload { args, receipt } => {
                let config = cloud_deploy::read_config(&args.config)?;
                (
                    "localDeploymentRetry",
                    cloud_deploy::retry_upload(&args.directory, &config, &args.state, receipt)?,
                )
            }
            Deploy::LinkScene {
                directory,
                scene,
                mapping,
                output,
            } => {
                let mapping = serde_json::from_slice(&std::fs::read(mapping)?)?;
                (
                    "localDeploymentLinking",
                    serde_json::to_value(roblox_toolchain::deployment::link_scene(
                        directory, scene, &mapping, output,
                    )?)?,
                )
            }
            Deploy::Upload(args) | Deploy::Publish(args) => {
                let config = cloud_deploy::read_config(&args.config)?;
                (
                    "cloudDeployment",
                    cloud_deploy::execute(
                        &args.directory,
                        &config,
                        &args.state,
                        matches!(command, Deploy::Publish(_)),
                    )?,
                )
            }
            Deploy::Status { state } => {
                let journal: cloud_deploy::Journal =
                    serde_json::from_slice(&std::fs::read(state.join("receipts.json"))?)?;
                ("localDeploymentReceipts", serde_json::to_value(journal)?)
            }
            Deploy::ReconcileOperation {
                args,
                receipt,
                operation_id,
            } => {
                let config = cloud_deploy::read_config(&args.config)?;
                (
                    "cloudDeploymentReconciliation",
                    cloud_deploy::reconcile_operation(
                        &args.directory,
                        &config,
                        &args.state,
                        receipt,
                        operation_id,
                    )?,
                )
            }
            Deploy::ReconcilePublication {
                args,
                sha256,
                version,
            } => {
                let config = cloud_deploy::read_config(&args.config)?;
                (
                    "cloudDeploymentReconciliation",
                    cloud_deploy::reconcile_publication(
                        &args.directory,
                        &config,
                        &args.state,
                        sha256,
                        *version,
                    )?,
                )
            }
        };
        return Ok(json!({"ok":true,"scope":scope,"result":result}));
    }
    let Command::Assets {
        plan: path,
        command,
    } = cli.command
    else {
        return Ok(json!({"ok":true,"result":{
            "commandGroups":["assets","convert","build","compile","deploy"],"assetOperations":["init","add","edit","remove","list","inspect","validate"],
            "deploymentOperations":["link-scene","upload","publish","status","retry-upload","reconcile-operation","reconcile-publication"],"cloudUpload":true,
            "assetDocument":"offlineBuildPlan",
            "localLuauCompilation":true,
            "offlineSceneSerialization":true,
            "offlineAssetBundleBuild":true,
            "offlineConversion":[
                "static-gltf-to-mesh-v2","static-fbx-to-mesh-v2","static-obj-to-mesh-v2",
                "textures-to-png-or-dds","material-to-surface-appearance-and-texturepack","gltf-material-to-surface-appearance",
                "terrain-to-smoothgrid-v1-and-optional-physicsgrid-v2",
                "collision-to-csgphs-v5","audio-to-ogg-vorbis","silent-video-to-webm-vp9",
                "combined-source-to-webm-vp9-vorbis","preconverted-media-to-webm",
                "canonical-animation-to-rbxm","rigid-linear-gltf-animation-to-rbxm",
                "sampled-fbx-animation-to-rbxm","rigid-bind-gltf-to-skinned-mesh-v4",
                "linear-fbx-to-skinned-mesh-v4"
            ],"offlineGameBuild":false,
            "persistentStudioImport":false,"studioCommandExecution":false
        }}));
    };
    asset_commands::execute(
        &path.ok_or("--plan is required for asset operations")?,
        command,
    )
}

fn main() -> ExitCode {
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(error) => {
            if !error.use_stderr() {
                print!("{error}");
                return ExitCode::SUCCESS;
            }
            eprintln!(
                "{}",
                json!({"ok":false,"error":{"code":"invalid_arguments","message":error.to_string()}})
            );
            return ExitCode::FAILURE;
        }
    };
    match execute(cli) {
        Ok(value) => {
            println!("{value}");
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!(
                "{}",
                json!({"ok":false,"error":{"code":"operation_failed","message":error.to_string()}})
            );
            ExitCode::FAILURE
        }
    }
}
