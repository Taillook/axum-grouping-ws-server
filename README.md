# axum-grouping-ws-server

This project is the example of grouping websocket server by [tokio-rs/axum](https://github.com/tokio-rs/axum).

## How to use
### Dependency
nats-server ^2.0.0

### docker-compose
`docker compose up`

## Operation example
user1
```
⫸ wscat -c "localhost:8088/websocket/group1/user1"
Connected (press CTRL+C to quit)
> hello!
< user1: hello!
< user2: yeah!
> 
```
user2
```
⫸ wscat -c "localhost:8088/websocket/group1/user2"
Connected (press CTRL+C to quit)
< user1: hello!
> yeah!
< user2: yeah!
> 
```
