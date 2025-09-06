use std::io::Result;

fn main() -> Result<()> {
    let mut config = prost_build::Config::new();
    
    // Add Eq derive only to message types, not enums (they already have it)
    config.type_attribute("landro.proto.Hello", "#[derive(Eq)]");
    config.type_attribute("landro.proto.FolderSummary", "#[derive(Eq)]");
    config.type_attribute("landro.proto.Want", "#[derive(Eq)]");
    config.type_attribute("landro.proto.Ack", "#[derive(Eq)]");
    config.type_attribute("landro.proto.Error", "#[derive(Eq)]");
    config.type_attribute("landro.proto.FileEntry", "#[derive(Eq)]");
    config.type_attribute("landro.proto.Manifest", "#[derive(Eq)]");
    config.type_attribute("landro.proto.ChunkData", "#[derive(Eq)]");
    config.out_dir("src/generated");
    config.compile_protos(&["proto/messages.proto"], &["proto/"])?;
    Ok(())
}
