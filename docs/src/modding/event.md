# Events
## register_event(id, handler: ||)
Adds event handler to list of event handlers for that id. When event with specified id is called, all event handlers registered on that id will get called with ```this``` variable set to event_data. ```this``` variable is shared with all handlers. Example: 
```rhai
register_event("bb:player_join", ||{
#   this.entity = Entity("core:player", this.location);
});
```
## call_event(id, event_data: any) -> any
Calls all event handlers with specified id passing them ```event_data``` as ```this```. This method function returns ```event_data``` after it passes all event handlers.