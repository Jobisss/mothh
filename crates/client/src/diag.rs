//! Linhas de diagnóstico que, além do log do próprio app, vão para o log do
//! servidor (`ClientMessage::Diagnostic`), onde dá para acompanhar todo mundo
//! num lugar só: chave da chamada, rota e qualidade da chamada, erros na
//! troca de chaves. Nunca o conteúdo de uma mensagem.

use std::sync::Mutex;

/// O que ainda não foi para o servidor. Com a conexão caída por muito
/// tempo, as mais velhas saem para não crescer sem fim.
static QUEUE: Mutex<Vec<String>> = Mutex::new(Vec::new());
const MAX_QUEUED: usize = 50;

/// Escreve no log do app e guarda para mandar ao servidor.
pub fn report(text: impl Into<String>) {
    let text = text.into();
    eprintln!("{text}");
    if let Ok(mut queue) = QUEUE.lock() {
        if queue.len() >= MAX_QUEUED {
            queue.remove(0);
        }
        queue.push(text);
    }
}

/// O que juntou desde a última vez.
pub fn take() -> Vec<String> {
    QUEUE
        .lock()
        .map(|mut queue| std::mem::take(&mut *queue))
        .unwrap_or_default()
}
