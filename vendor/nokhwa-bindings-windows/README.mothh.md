# nokhwa-bindings-windows 0.4.6 com correção do mothh

Cópia da crate do crates.io, usada pelo `[patch.crates-io]` do Cargo.toml da raiz.

Única mudança (`src/lib.rs`, função `frame_rate_of`): a taxa de quadros do
Media Foundation é uma fração (MFRatio). O original zerava toda taxa com
denominador diferente de 1, então câmeras que só oferecem 30000/1001 ou
60060/1001 (a Iriun Webcam, por exemplo) ficavam sem nenhum formato e não
abriam ("Failed to fulfill requested format"). Agora a fração é arredondada
(29,97 vira 30). O `format_refreshed` também lia o denominador como taxa; usa
a mesma função.

Ao subir a versão do nokhwa, ver se a correção já entrou lá e apagar esta pasta.
