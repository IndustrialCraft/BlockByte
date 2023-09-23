use crate::texture::pack_textures;
use block_byte_common::content::ClientContent;
use image::RgbaImage;
use std::collections::HashMap;
use std::path::Path;

pub fn load_assets(zip_path: &Path) -> (RgbaImage,) {
    let mut zip =
        zip::ZipArchive::new(std::fs::File::open(zip_path).expect("asset archive not found"))
            .expect("asset archive invalid");
    let mut textures_to_pack = Vec::new();
    let mut models = HashMap::new();

    let mut content = None;
    let mut font = None;

    for file in 0..zip.len() {
        let mut file = zip.by_index(file).unwrap();
        if !file.is_file() {
            continue;
        }
        let mut data = Vec::new();
        use std::io::Read;
        file.read_to_end(&mut data).unwrap();
        let name = file.name();
        if name.ends_with(".png") {
            textures_to_pack.push((name.replace(".png", ""), data));
            continue;
        }
        if name.ends_with(".wav") {
            //todo
            //sound_manager.load(name.replace(".wav", ""), data);
            continue;
        }
        if name.ends_with(".bbm") {
            models.insert(name.replace(".bbm", ""), data);
            continue;
        }
        if name == "content.json" {
            content = Some(serde_json::from_str::<ClientContent>(
                String::from_utf8(data).unwrap().as_str(),
            ));
            continue;
        }
        if name == "font.ttf" {
            font = Some(rusttype::Font::try_from_vec(data).unwrap());
            continue;
        }
    }
    let font = font.unwrap();
    let (texture_atlas, texture_image) = pack_textures(textures_to_pack, &font);
    //let content = load_content(content.unwrap(), &texture_atlas, &texture, models);
    (texture_image,)
}
