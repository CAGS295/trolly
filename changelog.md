# Goals
- Have a command to build a global order book.

## WIP

- optimize allocations
- prometheus server.
- add a DockerFile
- serve through grpc
- add multiple readers/single writer for a monitor.

## change log
+ 
+ decouple web socket base url from the event subscription.
+ define a new subcommand depth for monitor
+ it should work as follows ./bin monitor <metric> [sources, symbol]
+ secure web socket streams
+ Graceful shutdown
+ pretty panics
+ basic logging
