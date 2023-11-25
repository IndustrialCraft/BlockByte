# Adding Blocks
## Example Block 
```
create_block(|p|{
    create_static("overworld:branch","overworld:log_side", transform_rotation_from_face(p.facing.to_face())).no_collide()
}).add_property_horizontal_face("facing").register("overworld:branch");
```
This will create a block with id ```overworld:branch``` with facing property. Client-side, it will be static model, with its model being ```overworld:branch``` and texture ```overworld:log_side``` rotated based on face property.
## Block states
Each block has at least 1 state. More states can be added with add_property_xxx calls. Remeber that number of states grows exponentially with amount of properties. 
## Methods
### create_block(client_state_generator: |properties| -> ClientBlock) -> BlockBuilder
### BlockBuilder::add_property_horizontal_face(name: string) -> Self
### BlockBuilder::add_property_face(name: string) -> Self
### BlockBuilder::add_property_bool(name: string) -> Self
### BlockBuilder::add_property_number(name: string, range: RangeInclusive) -> Self
### BlockBuilder::breaking_tool(tool: ToolType, hardness: float) -> Self
### BlockBuilder::ticker(ticker: |block: WorldBlock|) -> Self
### BlockBuilder::right_click_action(action: |block: WorldBlock, player: Player|) -> Self
### BlockBuilder::neighbor_update(updater: |location: BlockLocation|) -> Self
### BlockBuilder::breaking_speed(speed: float) -> Self
### BlockBuilder::data_container(inventory_size: integer) -> Self
### BlockBuilder::register(id) -> RegisteredBlock
### RegisteredBlock::register_item(item_id: id, item_name: string)
### create_air() -> ClientBlock
### create_cube(front: id, back: id, left: id, right: id, up: id, down: id) -> ClientBlock
### create_static(model: id, texture: id) -> ClientBlock
### create_static(model: id, texture: id, transform: Transform) -> ClientBlock
### create_foliage(texture_1: id, texture_2: id, texture_3: id, texture_4: id) -> ClientBlock
### ClientBlock::add_static_model(model: id, texture: id, transform: Transform)
### ClientBlock::fluid(fluid: bool)
### ClientBlock::no_collide()
### ClientBlock::transparent(transparent: bool)
### ClientBlock::selectable(selectable: bool)
### ClientBlock::render_data(render_data: integer)
### ClientBlock::dynamic(model: id, texture: id)
### ClientBlock::dynamic_add_animation(animation: string)
### ClientBlock::dynamic_add_item(item: string)