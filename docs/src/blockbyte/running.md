# Running BlockByte Client & Server
## Running server
Use cargo run to start server: ```cargo run --bin block_byte_server --release```.  
In working directory you must provide mods folder, from which server will load mods.  
Upon loading successfully, server will print ```server started``` message, create saves directory and start listening on port 4321.  
To stop running server, you can use ctrl+c. Pressing it first time will try to stop server gracefully, saving world and kicking plyers. Pressing it second time will forcefully kill the server.
## Server Config
After stopping server, a file in saves directory is created named ```settings.txt```. It has format ```path.to.property=value```. When you change values, they get automatically loaded at next server startup. Do not change this file while server is running, as it will get overridden once server stops.
## Running Client
Use cargo to start client: ```cargo run --bin block_byte_client --release -- [path to content] [ip]:[port]```  
You can obtain content either by asking server for it(todo: protocol) or from server's saves directory, where server dumps it as ```content.zip```  
