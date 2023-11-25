# Creating a mod
Upon server startup, BlockByte will load the mod directory. In order to create a valid mod it has to have descriptor.json file. Here is an example descriptor: 
```
{
    "id": "test",
    "name": "Test Mod",
    "description": "test mod",
    "authors": ["your name"]
}
```
To add content to your mod, create these folders:
- images - stores images
- gui - stores gui layouts
- models - stores models
- scripts - stores scripts
- sounds - stores sounds
- structures - stores structures
- tags - stores tag list
## Scripts
BlockByte will run all files in script folder and it's subfolders on startup. They should end in ```.rhs``` as they are [rhai](https://rhai.rs/) source files.