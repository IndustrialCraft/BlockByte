# Adding Items
## Example Item 
```rhai
create_item().on_right_click(|player,target|{
#    if type_of(target) == "BlockPosition"{
#        if !player.get_entity().is_shifting(){
#            player.user_data[Identifier("core:first_selection")] = target;
#            player.send_chat_message("first point selected at " + target);
#        } else {
#            player.user_data[Identifier("core:second_selection")] = target;
#            player.send_chat_message("second point selected at " + target);
#        }
#    }
}).client_name("Selection Wand").register("core:selection_wand");
```
This will create item with id ```core:selection_wand```, name ```Selection Wand``` and right click handler
## Methods
### create_item() -> ItemBuilder
### ItemBuilder::tool(durability: number, speed: float, hardness: float) -> Self
### ItemBuilder::tool_add_type(type: ToolType) -> Self
### ItemBuilder::client_name(name: string) -> Self
### ItemBuilder::client_model_texture(texture: id) -> Self
### ItemBuilder::client_model_block(block: id) -> Self
### ItemBuilder::place(block: id) -> Self
### ItemBuilder::on_right_click(player: Player, target: [BlockPosition/()]) -> Self
### ItemBuilder::stack_size(size: number) -> Self
### ItemBuilder::register(id)