// Compila a interface (`ui/app.slint`) para código Rust durante o build.
fn main() {
    slint_build::compile("ui/app.slint").expect("erro ao compilar ui/app.slint");
}
