# Goals
- Have a command to build a global order book.

## WIP

- optimize allocations
- prometheus server.
- serve through grpc
    - graceful reader shutdown
    - single writer, multiple readers
    - add concurrent reads to the LOB
    - add multiple readers/single writer for a monitor.
    - serve depth reads with tonic.
    - reader client tests
    - add depth options to control reader server

## change log
+ add a DockerFile
+ Serve the LOB through gRPC.
+ decouple web socket base url from the event subscription.
+ define a new subcommand depth for monitor
+ it should work as follows ./bin monitor <metric> [sources, symbol]
+ secure web socket streams
+ Graceful shutdown
+ pretty panics
+ basic logging
