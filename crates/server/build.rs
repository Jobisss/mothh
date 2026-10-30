// Recompila o servidor quando uma migration muda, porque `sqlx::migrate!`
// copia os arquivos .sql para dentro do binário.
fn main() {
    println!("cargo:rerun-if-changed=migrations");
}
