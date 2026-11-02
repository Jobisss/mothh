//! Inspeciona os dois GLBs sem iniciar rede, login ou o cliente completo.
#[path = "../src/turntable_mesh.rs"]
mod turntable_mesh;
fn main() {
    let directory = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "target/turntable-preview".into());
    std::fs::create_dir_all(&directory).unwrap();
    for (name, bytes) in [
        ("disco", turntable_mesh::PRIMARY),
        (
            "disco-2",
            include_bytes!("../../../assets/models/disco-2.glb").as_slice(),
        ),
    ] {
        let scene = turntable_mesh::Scene::load(bytes).unwrap();
        let rgba = scene.render(640, 600, turntable_mesh::YAW, turntable_mesh::PITCH, 0.0);
        image::save_buffer(
            format!("{directory}/{name}.png"),
            &rgba,
            640,
            600,
            image::ColorType::Rgba8,
        )
        .unwrap();
        let mut pixel = scene.render(96, 90, turntable_mesh::YAW, turntable_mesh::PITCH, 0.0);
        turntable_mesh::pixel_filter(&mut pixel, 96);
        image::save_buffer(
            format!("{directory}/{name}-pixel.png"),
            &pixel,
            96,
            90,
            image::ColorType::Rgba8,
        )
        .unwrap();
    }
}
