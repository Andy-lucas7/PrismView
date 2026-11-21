# Regras PrismView & Economia
- Respostas ultra-curtas. Sem saudações, introduções ou resumos longos.
- Nunca leia `node_modules/`, `target/`, `.git/`, `dist/`, `.glb`, `.gltf`, `.png`, `.jpg`, `package-lock.json`.
- Leia apenas os arquivos estritamente necessários para a tarefa atual e aplique patches pontuais.
- Regras técnicas: nunca alocar objetos/vetores dentro de loops de render 60–144 FPS; sempre chamar `dispose()` em geometrias, materiais e texturas 3D descartadas (exceto texturas compartilhadas em cache).
