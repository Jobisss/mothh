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
