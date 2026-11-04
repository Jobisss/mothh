# Perfil

O perfil é o que os outros veem de você: foto, nome e, na fase 4, os itens comemorativos que você escolher mostrar.

## Hoje (v0.3.6)

- **Página de perfil**: abre clicando na sua foto ou no seu nome, no rodapé da barra lateral. Mostra como você aparece na lista de conversas dos outros e um cartão com a foto grande, o nome e o @usuário.
- **Foto**: PNG, JPG ou WEBP; o app recorta o meio quadrado e manda um PNG de 128 px para o servidor. Quem divide uma conversa com você vê a foto na lista, na faixa da chamada e em quem está em chamada.
- **Nome**: é também o nome de entrar. Trocar avisa na hora quem divide uma conversa com você.
- **Senha**: pede a atual; os outros aparelhos saem da conta.

## Conta: e-mail e senha (v0.6)

- **E-mail no cadastro**: conta nova pede usuário, e-mail e senha. Contas de antes continuam entrando sem e-mail e podem pôr um no perfil (seção E-MAIL), que é o que permite recuperar a senha.
- **Entrar** aceita o usuário ou o e-mail, e o campo de senha tem "ver"/"ocultar".
- **Esqueci a senha**: a pessoa informa o e-mail e recebe um código de seis dígitos, que vale 15 minutos e cinco tentativas. Com o código define a senha nova; todas as sessões da conta caem. O servidor responde igual exista a conta ou não. Enquanto não há SMTP configurado, o código sai no log do servidor (`crates/server/src/mail.rs`), e quem opera repassa à pessoa.

## Fase 4: itens comemorativos

Os itens comemorativos em 3D do [inventário](inventario.md) aparecem no perfil: a pessoa escolhe quais mostrar, e quem abre o perfil vê esses itens.
