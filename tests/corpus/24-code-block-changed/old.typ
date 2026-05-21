Here is the original implementation:

```rust
fn greet() -> &'static str {
    "Hello, world!"
}

fn main() {
    println!("{}", greet());
}
```

The function above prints a fixed greeting.
