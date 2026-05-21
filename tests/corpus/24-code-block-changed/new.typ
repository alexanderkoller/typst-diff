Here is the updated implementation:

```rust
fn greet(name: &str) -> String {
    format!("Hello, {}!", name)
}

fn main() {
    println!("{}", greet("Alice"));
}
```

The function above accepts a name parameter.
