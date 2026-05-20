mod diff;
mod eval;
mod world;
pub use eval::eval_to_content;
pub use world::SystemWorld;

fn main() {
    println!("typst-diff");
}
