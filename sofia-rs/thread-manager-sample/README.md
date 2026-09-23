# ThreadManager Sample

Small one-shot binary that starts a Sofia thread with `ThreadManager` from
`sofia-core-api`, submits a single user turn, and prints the final assistant
message.

```sh
cargo run -p sofia-thread-manager-sample -- "Say hello"
```

Use `--model` to override the configured default model:

```sh
cargo run -p sofia-thread-manager-sample -- --model gpt-5.2 "Say hello"
```

The prompt can also be piped through stdin:

```sh
printf 'Say hello\n' | cargo run -p sofia-thread-manager-sample
```
