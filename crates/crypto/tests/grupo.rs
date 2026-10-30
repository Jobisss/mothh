//! O que a F2-07 promete: entrar, sair, cifrar, decifrar, e quem saiu não
//! lê mais nada.

use std::path::PathBuf;

use crypto::{Error, Identity, MemoryVault, Received};

fn text(received: Received) -> (String, String) {
    match received {
        Received::Text { from, text } => (from, String::from_utf8(text).unwrap()),
        other => panic!("esperava texto, veio {other:?}"),
    }
}

#[test]
fn tres_pessoas_conversam_e_quem_sai_nao_le_mais() {
    let ana = Identity::in_memory("ana").unwrap();
    let bia = Identity::in_memory("bia").unwrap();
    let caio = Identity::in_memory("caio").unwrap();

    // Ana cria o grupo e convida as duas pelos pacotes de chaves delas.
    let mut group_ana = ana.create_group(b"conversa-1").unwrap();
    assert_eq!(group_ana.members(), ["ana"]);
    let invitation = group_ana
        .add(
            &ana,
            &[bia.key_package().unwrap(), caio.key_package().unwrap()],
        )
        .unwrap();
    let mut group_bia = bia.join(&invitation.welcome).unwrap();
    let mut group_caio = caio.join(&invitation.welcome).unwrap();
    assert_eq!(group_ana.members(), ["ana", "bia", "caio"]);
    assert_eq!(group_bia.members(), group_ana.members());
    assert_eq!(group_caio.members(), group_ana.members());
    assert_eq!(group_bia.id(), b"conversa-1");

    // Ana fala; as duas leem.
    let sealed = group_ana.encrypt(&ana, "oi gente".as_bytes()).unwrap();
    assert_eq!(
        text(group_bia.receive(&bia, &sealed).unwrap()),
        ("ana".into(), "oi gente".into())
    );
    assert_eq!(
        text(group_caio.receive(&caio, &sealed).unwrap()),
        ("ana".into(), "oi gente".into())
    );

    // Caio responde; Ana e Bia leem.
    let sealed = group_caio.encrypt(&caio, "oi!".as_bytes()).unwrap();
    assert_eq!(
        text(group_ana.receive(&ana, &sealed).unwrap()),
        ("caio".into(), "oi!".into())
    );
    assert_eq!(
        text(group_bia.receive(&bia, &sealed).unwrap()),
        ("caio".into(), "oi!".into())
    );

    // Ana remove Caio. O commit vai para todo mundo, inclusive Caio.
    let commit = group_ana.remove(&ana, "caio").unwrap();
    assert_eq!(
        group_bia.receive(&bia, &commit).unwrap(),
        Received::Commit {
            members: vec!["ana".into(), "bia".into()]
        }
    );
    assert_eq!(group_bia.members(), ["ana", "bia"]);
    // Caio fica sabendo que saiu e o grupo dele morre.
    assert!(group_caio.is_active());
    let _ = group_caio.receive(&caio, &commit);
    assert!(!group_caio.is_active());

    // Daqui em diante Caio não decifra mais nada.
    let sealed = group_ana
        .encrypt(&ana, "agora só nós duas".as_bytes())
        .unwrap();
    assert_eq!(
        text(group_bia.receive(&bia, &sealed).unwrap()),
        ("ana".into(), "agora só nós duas".into())
    );
    assert!(matches!(
        group_caio.receive(&caio, &sealed),
        Err(Error::Inactive)
    ));
    assert!(matches!(
        group_caio.encrypt(&caio, b"ainda estou aqui?"),
        Err(Error::Inactive)
    ));
}

#[test]
fn quem_nunca_entrou_nao_le_mesmo_antes_de_ser_removido() {
    let ana = Identity::in_memory("ana").unwrap();
    let bia = Identity::in_memory("bia").unwrap();
    let dani = Identity::in_memory("dani").unwrap();
    let mut group_ana = ana.create_group(b"c").unwrap();
    let invitation = group_ana.add(&ana, &[bia.key_package().unwrap()]).unwrap();
    let mut group_bia = bia.join(&invitation.welcome).unwrap();

    // Dani está num grupo com o mesmo id, mas não é este grupo.
    let mut group_dani = dani.create_group(b"c").unwrap();
    let sealed = group_ana.encrypt(&ana, b"segredo").unwrap();
    assert!(group_dani.receive(&dani, &sealed).is_err());
    assert!(group_bia.receive(&bia, &sealed).is_ok());
}

#[test]
fn o_cifrado_nao_contem_o_texto() {
    let ana = Identity::in_memory("ana").unwrap();
    let bia = Identity::in_memory("bia").unwrap();
    let mut group_ana = ana.create_group(b"c").unwrap();
    let invitation = group_ana.add(&ana, &[bia.key_package().unwrap()]).unwrap();
    let _ = bia.join(&invitation.welcome).unwrap();

    let plain = b"uma frase bem comprida para procurar depois";
    let sealed = group_ana.encrypt(&ana, plain).unwrap();
    assert!(!sealed.windows(plain.len()).any(|w| w == plain));
    // Duas vezes a mesma frase, dois cifrados diferentes.
    let again = group_ana.encrypt(&ana, plain).unwrap();
    assert_ne!(sealed, again);
}

#[test]
fn remover_quem_nao_esta_da_erro() {
    let ana = Identity::in_memory("ana").unwrap();
    let mut group = ana.create_group(b"c").unwrap();
    assert!(matches!(group.remove(&ana, "zeca"), Err(Error::NotMember(id)) if id == "zeca"));
    assert!(matches!(group.add(&ana, &[]), Err(Error::Message(_))));
    assert!(matches!(
        group.add(&ana, &[b"lixo".to_vec()]),
        Err(Error::Message(_))
    ));
    assert!(matches!(ana.join(b"lixo"), Err(Error::Message(_))));
}

/// Uma pasta temporária só deste teste.
fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "mothh-crypto-{name}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn estado_salvo_continua_no_grupo_depois_de_reabrir() {
    let dir = temp_dir("estado");
    let file = dir.join("mls.bin");
    let vault = MemoryVault::new();

    let bia = Identity::in_memory("bia").unwrap();
    let mut group_bia;
    {
        let ana = Identity::open(&vault, "ana", &file).unwrap();
        let mut group_ana = ana.create_group(b"c").unwrap();
        let invitation = group_ana.add(&ana, &[bia.key_package().unwrap()]).unwrap();
        group_bia = bia.join(&invitation.welcome).unwrap();
        let sealed = group_ana.encrypt(&ana, b"antes de fechar").unwrap();
        assert_eq!(
            text(group_bia.receive(&bia, &sealed).unwrap()).1,
            "antes de fechar"
        );
        ana.save().unwrap();
    }
    // O arquivo não mostra nada em claro.
    let bytes = std::fs::read(&file).unwrap();
    assert!(!bytes.windows(3).any(|w| w == b"ana"));

    // O app abre de novo: mesma identidade, mesmo grupo, e Bia continua
    // conseguindo falar com ela.
    let ana = Identity::open(&vault, "ana", &file).unwrap();
    assert_eq!(ana.user_id(), "ana");
    let mut group_ana = ana.load_group(b"c").unwrap().expect("grupo salvo");
    assert_eq!(group_ana.members(), ["ana", "bia"]);
    let sealed = group_bia.encrypt(&bia, b"depois de abrir").unwrap();
    assert_eq!(
        text(group_ana.receive(&ana, &sealed).unwrap()),
        ("bia".into(), "depois de abrir".into())
    );
    let sealed = group_ana.encrypt(&ana, b"de volta").unwrap();
    assert_eq!(
        text(group_bia.receive(&bia, &sealed).unwrap()).1,
        "de volta"
    );
    assert!(ana.load_group(b"outro").unwrap().is_none());

    // Com outro cofre (outra chave), o arquivo não abre.
    let other = MemoryVault::new();
    assert!(matches!(
        Identity::open(&other, "ana", &file),
        Err(Error::Corrupted)
    ));

    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn pacote_de_chaves_salvo_serve_para_entrar_depois_de_reabrir() {
    let dir = temp_dir("pacote");
    let file = dir.join("mls.bin");
    let vault = MemoryVault::new();

    // Bia publica um pacote e fecha o app.
    let package = {
        let bia = Identity::open(&vault, "bia", &file).unwrap();
        let package = bia.key_package().unwrap();
        bia.save().unwrap();
        package
    };
    let ana = Identity::in_memory("ana").unwrap();
    let mut group_ana = ana.create_group(b"c").unwrap();
    let invitation = group_ana.add(&ana, &[package]).unwrap();

    // Ao abrir de novo, o welcome ainda encaixa no pacote salvo.
    let bia = Identity::open(&vault, "bia", &file).unwrap();
    let mut group_bia = bia.join(&invitation.welcome).unwrap();
    let sealed = group_ana.encrypt(&ana, b"bem-vinda").unwrap();
    assert_eq!(
        text(group_bia.receive(&bia, &sealed).unwrap()).1,
        "bem-vinda"
    );

    std::fs::remove_dir_all(&dir).unwrap();
}

/// O que a F2-08 usa: a mudança fica pendente até o servidor aceitar, e quem
/// perde a corrida descarta a sua e aplica a do outro.
#[test]
fn mudanca_pendente_espera_o_servidor_e_quem_perde_descarta() {
    let ana = Identity::in_memory("ana").unwrap();
    let bia = Identity::in_memory("bia").unwrap();
    let caio = Identity::in_memory("caio").unwrap();
    let dani = Identity::in_memory("dani").unwrap();
    let mut group_ana = ana.create_group(b"c").unwrap();
    let invitation = group_ana.add(&ana, &[bia.key_package().unwrap()]).unwrap();
    let mut group_bia = bia.join(&invitation.welcome).unwrap();
    assert_eq!(group_ana.epoch(), 1);

    // As duas querem pôr alguém ao mesmo tempo, na época 1.
    let from_ana = group_ana
        .commit(&ana, &[caio.key_package().unwrap()], &[])
        .unwrap();
    let from_bia = group_bia
        .commit(&bia, &[dani.key_package().unwrap()], &[])
        .unwrap();
    assert!(from_ana.welcome.is_some());
    assert!(group_ana.has_pending_commit());
    // Com mudança pendente, o grupo não cifra.
    assert!(group_ana.encrypt(&ana, b"agora?").is_err());

    // O servidor aceita a da Ana. Bia recebe e a dela é descartada.
    group_ana.confirm(&ana).unwrap();
    assert_eq!(
        group_bia.receive(&bia, &from_ana.commit).unwrap(),
        Received::Commit {
            members: vec!["ana".into(), "bia".into(), "caio".into()]
        }
    );
    assert!(!group_bia.has_pending_commit());
    let mut group_caio = caio.join(&from_ana.welcome.unwrap()).unwrap();
    // O welcome da Bia nunca é entregue: o servidor recusou o commit dela.
    drop(from_bia);
    assert_eq!(group_ana.epoch(), 2);
    assert_eq!(group_bia.epoch(), 2);

    // Todo mundo na mesma época conversa.
    let sealed = group_bia.encrypt(&bia, b"todas aqui").unwrap();
    assert_eq!(
        text(group_ana.receive(&ana, &sealed).unwrap()).1,
        "todas aqui"
    );
    assert_eq!(
        text(group_caio.receive(&caio, &sealed).unwrap()).1,
        "todas aqui"
    );

    // Descartar à toa (sem nada pendente) não quebra nada.
    group_ana.discard(&ana).unwrap();
    let sealed = group_ana.encrypt(&ana, b"ok").unwrap();
    assert_eq!(text(group_bia.receive(&bia, &sealed).unwrap()).1, "ok");
}

#[test]
fn mensagem_de_epoca_futura_espera_e_depois_abre() {
    let ana = Identity::in_memory("ana").unwrap();
    let bia = Identity::in_memory("bia").unwrap();
    let caio = Identity::in_memory("caio").unwrap();
    let mut group_ana = ana.create_group(b"c").unwrap();
    let invitation = group_ana.add(&ana, &[bia.key_package().unwrap()]).unwrap();
    let mut group_bia = bia.join(&invitation.welcome).unwrap();

    let invitation = group_ana.add(&ana, &[caio.key_package().unwrap()]).unwrap();
    let sealed = group_ana.encrypt(&ana, b"depois do commit").unwrap();
    // Bia ainda não passou o commit: a mensagem é do futuro para ela.
    assert!(matches!(
        group_bia.receive(&bia, &sealed),
        Err(Error::FutureEpoch)
    ));
    group_bia.receive(&bia, &invitation.commit).unwrap();
    assert_eq!(
        text(group_bia.receive(&bia, &sealed).unwrap()).1,
        "depois do commit"
    );
}

#[test]
fn mensagem_atrasada_de_epoca_passada_ainda_abre() {
    let ana = Identity::in_memory("ana").unwrap();
    let bia = Identity::in_memory("bia").unwrap();
    let caio = Identity::in_memory("caio").unwrap();
    let mut group_ana = ana.create_group(b"c").unwrap();
    let invitation = group_ana.add(&ana, &[bia.key_package().unwrap()]).unwrap();
    let mut group_bia = bia.join(&invitation.welcome).unwrap();

    // Bia escreveu na época 1, mas a mensagem chega depois de Ana pôr Caio.
    let late = group_bia.encrypt(&bia, b"atrasada").unwrap();
    let invitation = group_ana.add(&ana, &[caio.key_package().unwrap()]).unwrap();
    group_bia.receive(&bia, &invitation.commit).unwrap();
    assert_eq!(text(group_ana.receive(&ana, &late).unwrap()).1, "atrasada");
}

#[test]
fn quem_perdeu_o_estado_sai_e_entra_de_novo_numa_mudanca_so() {
    let ana = Identity::in_memory("ana").unwrap();
    let bia = Identity::in_memory("bia").unwrap();
    let mut group_ana = ana.create_group(b"c").unwrap();
    let invitation = group_ana.add(&ana, &[bia.key_package().unwrap()]).unwrap();
    let _old_bia = bia.join(&invitation.welcome).unwrap();

    // Bia reinstalou o app: identidade nova, mesmo usuário.
    let bia = Identity::in_memory("bia").unwrap();
    let pending = group_ana
        .commit(&ana, &[bia.key_package().unwrap()], &["bia".to_string()])
        .unwrap();
    group_ana.confirm(&ana).unwrap();
    assert_eq!(group_ana.members(), ["ana", "bia"]);
    let mut group_bia = bia.join(&pending.welcome.unwrap()).unwrap();
    let sealed = group_ana.encrypt(&ana, b"de volta").unwrap();
    assert_eq!(
        text(group_bia.receive(&bia, &sealed).unwrap()).1,
        "de volta"
    );

    // Nada para mudar é erro; tirar a si mesma é ignorado.
    assert!(group_ana.commit(&ana, &[], &["ana".to_string()]).is_err());
}

#[test]
fn segredo_exportado_e_igual_para_todos_e_muda_com_a_epoca() {
    let ana = Identity::in_memory("ana").unwrap();
    let bia = Identity::in_memory("bia").unwrap();
    let caio = Identity::in_memory("caio").unwrap();
    let mut group_ana = ana.create_group(b"c").unwrap();
    let invitation = group_ana.add(&ana, &[bia.key_package().unwrap()]).unwrap();
    let mut group_bia = bia.join(&invitation.welcome).unwrap();

    let key = group_ana.export_secret(&ana, "mothh sframe", 32).unwrap();
    assert_eq!(key.len(), 32);
    assert_eq!(
        key,
        group_bia.export_secret(&bia, "mothh sframe", 32).unwrap()
    );
    assert_ne!(
        key,
        group_ana.export_secret(&ana, "outra coisa", 32).unwrap()
    );

    let invitation = group_ana.add(&ana, &[caio.key_package().unwrap()]).unwrap();
    group_bia.receive(&bia, &invitation.commit).unwrap();
    let next = group_ana.export_secret(&ana, "mothh sframe", 32).unwrap();
    assert_ne!(key, next);
    assert_eq!(
        next,
        group_bia.export_secret(&bia, "mothh sframe", 32).unwrap()
    );
}

#[test]
fn apagar_grupo_e_cifra_local() {
    let ana = Identity::in_memory("ana").unwrap();
    let _ = ana.create_group(b"c").unwrap();
    assert!(ana.load_group(b"c").unwrap().is_some());
    ana.delete_group(b"c").unwrap();
    assert!(ana.load_group(b"c").unwrap().is_none());
    ana.delete_group(b"nunca-existiu").unwrap();
    // Dá para criar de novo com o mesmo id.
    let _ = ana.create_group(b"c").unwrap();

    let sealed = ana.seal_local(b"texto lido").unwrap();
    assert!(!sealed.windows(5).any(|w| w == b"texto"));
    assert_eq!(ana.open_local(&sealed).unwrap(), b"texto lido");
    let other = Identity::in_memory("ana").unwrap();
    assert!(matches!(other.open_local(&sealed), Err(Error::Corrupted)));
}
