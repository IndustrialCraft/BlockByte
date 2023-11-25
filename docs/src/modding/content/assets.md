# Adding Assets
## Example
```rhai
register_model("core:player", "models/player.bbm");
register_sound("core:equip", "sounds/equip.wav");

register_image("core:player", "images/player.png");
register_image("overworld:savanna_grass_side", load_image("images/dirt.png").overlay(load_image("images/grass_side_mask.png").multiply(savanna_grass_colored)));
```
## Methods
### register_model(id, path)
### register_sound(id, path)
### register_image(id, path)
### register_image(id, Image)
### load_image(path) -> Image
### Image::multiply(other: Image) -> Self
### Image::overlay(other: Image) -> Self
### Image::color(Color) -> Self
### create_color(r: float, g: float, b: float, a: float) -> Color