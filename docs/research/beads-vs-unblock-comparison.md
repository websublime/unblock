# Beads CLI vs Unblock — Feature Comparison

**Objective:** Gap analysis feature-by-feature. Para cada funcionalidade do Beads, avaliar se o Unblock já cobre, se o GitHub tem primitiva nativa, e se podemos implementar no MCP.

| Legend | Meaning |
|---|---|
| ✅ | Temos / GitHub tem / MCP pode |
| ⚠️ | Parcialmente coberto |
| ❌ | Não temos / Não existe / Não aplicável |
| N/A | Não aplicável à nossa arquitectura |

---

## 1. Issue Management

| # | Beads Feature | Beads Command | Unblock Equivalente | GitHub Nativo | MCP Viável | Notas |
|---|---|---|---|---|---|---|
| 1.1 | Criar issue | `bd create "title" -t bug -p 1 -d "desc"` | ✅ `create` com title, type, priority, body | ✅ REST + Projects V2 | — | Paridade total |
| 1.2 | Criar com ID explícito | `bd create --id worker1-100` | ❌ | ❌ GitHub atribui número auto | ❌ | Não faz sentido — GitHub numera sequencialmente. Beads precisa disto por usar SQLite local |
| 1.3 | Criar com labels | `bd create -l bug,critical` | ✅ `create` com `labels` array | ✅ REST | — | Paridade total |
| 1.4 | Criar de ficheiro markdown | `bd create -f feature-plan.md` | ❌ | ❌ | ✅ | **Gap interessante.** MCP tool `create_from_plan` que parseia um .md e cria N issues com deps. Útil para agents que geram planos |
| 1.5 | Body de ficheiro | `bd create --body-file=desc.md` | ⚠️ `create` aceita `body` string | ✅ REST (body param) | ✅ | O agent já pode ler ficheiro e passar como string. Não precisa de flag especial — é pattern do CLI, não do MCP |
| 1.6 | Body de stdin | `bd create --body-file=-` | N/A | N/A | N/A | Pattern CLI, não aplicável a MCP (stdio é o transport do protocolo) |
| 1.7 | Criar epic com filhos hierárquicos | `bd create --parent bd-a3f8e9` | ✅ `create` com `parent` (Sub-Issues API) | ✅ Sub-Issues nativo | — | GitHub Sub-Issues (GA 2025). Paridade total |
| 1.8 | Criar com dep inline | `bd create --deps discovered-from:id` | ✅ `create` com `blocked_by` array | ✅ Blocking API nativo | — | Unblock suporta `blocked_by` no create. Tipo `discovered-from` não existe — ver §3 |
| 1.9 | Update fields | `bd update <id> --status --priority` | ✅ `update` com todos os campos | ✅ REST + Projects V2 | — | Paridade total. Unblock suporta `body_section` editing que Beads não tem |
| 1.10 | Update batch (múltiplos IDs) | `bd update id1 id2 id3 --priority 0` | ❌ | ✅ REST (N calls) | ✅ | **Gap.** Adicionar array `ids` aos params de `update`, `close`, `reopen`. MCP itera internamente |
| 1.11 | Edit no $EDITOR | `bd edit <id> --title --design` | N/A | ✅ GitHub UI | N/A | Pattern humano, não para agents. Agents usam `update` com `body_section` |
| 1.12 | Close com reason | `bd close <id> --reason "Done"` | ✅ `close` com `reason` | ✅ REST | — | Paridade total |
| 1.13 | Close batch | `bd close id1 id2 id3 --reason` | ❌ | ✅ REST (N calls) | ✅ | **Gap.** Mesmo que 1.10 — batch IDs |
| 1.14 | Reopen | `bd reopen <id> --reason` | ✅ `reopen` com `reason` | ✅ REST | — | Paridade total |
| 1.15 | Reopen batch | `bd reopen id1 id2 id3` | ❌ | ✅ REST (N calls) | ✅ | **Gap.** Mesmo que 1.10 |
| 1.16 | Show issue | `bd show <id> --json` | ✅ `show` com comments, deps, body sections | ✅ GraphQL | — | Unblock tem mais detalhe (parsed body sections) |
| 1.17 | Show múltiplos | `bd show id1 id2 --json` | ❌ | ✅ GraphQL | ✅ | **Gap menor.** Adicionar array `ids` ao `show` |

---

## 2. Ready / Find Work

| # | Beads Feature | Beads Command | Unblock Equivalente | GitHub Nativo | MCP Viável | Notas |
|---|---|---|---|---|---|---|
| 2.1 | Find ready work | `bd ready --json` | ✅ `ready` com filtros | ❌ (não computa grafo) | — | Core feature. Unblock mais avançado com graph engine |
| 2.2 | Stale issues | `bd stale --days 30 --status open` | ❌ | ⚠️ `updated:` search qualifier | ✅ | **Gap útil.** Tool `stale` que filtra por `updated_at < N days`. GitHub search API suporta `updated:<date`. MCP computa sobre cache |
| 2.3 | Claim issue | `bd update <id> --status in_progress` | ✅ `claim` (atómico) | ⚠️ Requires N field updates | — | Unblock superior — `claim` é atómico: status + agent + timestamp + comment num só tool call |

---

## 3. Dependencies

| # | Beads Feature | Beads Command | Unblock Equivalente | GitHub Nativo | MCP Viável | Notas |
|---|---|---|---|---|---|---|
| 3.1 | Blocking dependency | `bd dep add <a> <b> --type blocks` | ✅ `depends` | ✅ `blockedBy` API nativo | — | Paridade total |
| 3.2 | Related (soft link) | `bd dep add --type related` | ❌ | ⚠️ Menções em body/comments | ⚠️ | GitHub não tem "related" formal. Convenção via menções (#42) no body. MCP poderia enforçar pattern mas sem query API |
| 3.3 | Parent-child | `bd dep add --type parent-child` | ✅ `create --parent` / Sub-Issues | ✅ Sub-Issues API | — | Paridade via Sub-Issues nativo |
| 3.4 | Discovered-from | `bd create --deps discovered-from:id` | ⚠️ `create --blocked_by` + comment | ⚠️ Blocking + comment | ⚠️ | Beads distingue `discovered-from` de `blocks`. Nós usamos `blocked_by` + comment "Discovered while working on #X". Não afecta ready queue (igual ao Beads). Podemos adoptar label convention `discovered-from:#42` |
| 3.5 | Dep tree view | `bd dep tree <id>` | ⚠️ `show --include_deps` | ✅ Blocking/blockedBy edges | ✅ | **Gap.** `show` retorna deps directas. Falta tool `dep_tree` que mostra árvore completa recursiva. Graph engine já tem os dados |
| 3.6 | Remove dep | Implícito | ✅ `dep_remove` | ✅ GraphQL mutation | — | Paridade total |
| 3.7 | Cycle detection | Implícito | ✅ `dep_cycles` | ❌ | — | Unblock superior — tool dedicada |

---

## 4. Labels

| # | Beads Feature | Beads Command | Unblock Equivalente | GitHub Nativo | MCP Viável | Notas |
|---|---|---|---|---|---|---|
| 4.1 | Add label | `bd label add <id> <label>` | ⚠️ Via `update --labels_add` | ✅ REST | — | Coberto mas sem tool dedicada. `update` faz tudo |
| 4.2 | Remove label | `bd label remove <id> <label>` | ⚠️ Via `update --labels_remove` | ✅ REST | — | Igual a 4.1 |
| 4.3 | Add label batch | `bd label add id1 id2 id3 label` | ❌ | ✅ REST (N calls) | ✅ | **Gap.** Batch IDs no `update` resolve isto |
| 4.4 | List labels de issue | `bd label list <id>` | ⚠️ Via `show` | ✅ REST | — | `show` já retorna labels |
| 4.5 | List all labels | `bd label list-all` | ❌ | ✅ REST `/repos/{owner}/{repo}/labels` | ✅ | **Gap menor.** Útil para agents saberem que labels existem. Endpoint REST simples |

---

## 5. State (Labels as Cache)

| # | Beads Feature | Beads Command | Unblock Equivalente | GitHub Nativo | MCP Viável | Notas |
|---|---|---|---|---|---|---|
| 5.1 | Query state dimension | `bd state <id> <dimension>` | ❌ | ⚠️ Labels `dim:value` | ✅ | Beads usa labels como state machine cache (`patrol:active`). Nós usamos **Projects V2 fields** para o mesmo — Status, Priority, Agent são campos tipados, não labels. A abordagem GitHub-native é superior |
| 5.2 | Set state | `bd set-state <id> dim=val --reason` | ⚠️ Via `update --status` | ✅ Projects V2 fields | — | Nosso `update` cobre os campos que importam. Para dimensões custom além dos 7 campos, labels são o caminho |
| 5.3 | List state dims | `bd state list <id>` | ❌ | ⚠️ Labels + Fields query | ✅ | **Gap menor.** `show` já retorna fields + labels. Um wrapper de convenience é possível mas não prioritário |

---

## 6. Filtering & Search

| # | Beads Feature | Beads Command | Unblock Equivalente | GitHub Nativo | MCP Viável | Notas |
|---|---|---|---|---|---|---|
| 6.1 | List com filtros | `bd list --status --priority --type` | ✅ `list` com filtros equivalentes | ✅ GraphQL | — | Paridade total |
| 6.2 | Filter by assignee | `bd list --assignee alice` | ✅ `list --assignee` | ✅ GraphQL | — | Paridade |
| 6.3 | Filter by ID list | `bd list --id bd-123,bd-456` | ❌ | ✅ GraphQL `nodes(ids:)` | ✅ | **Gap menor.** Útil para agents que querem detalhes de N issues específicas. `show` multi-ID resolve |
| 6.4 | Label AND filter | `bd list --label bug,critical` | ✅ `list --label` (AND implícito) | ✅ GraphQL | — | Paridade |
| 6.5 | Label OR filter | `bd list --label-any frontend,backend` | ❌ | ⚠️ GitHub Search OR | ✅ | **Gap.** `list` filtro `label_any` com lógica OR. Implementável no graph cache filter |
| 6.6 | Title search | `bd list --title "auth"` | ✅ `search` (GitHub Search API) | ✅ Search API | — | Coberto por `search` tool |
| 6.7 | Desc/notes search | `bd list --desc-contains "impl"` | ⚠️ `search` faz full-text | ✅ Search API `in:body` | — | GitHub Search API pesquisa em body. Sem granularidade por secção |
| 6.8 | Date range filters | `bd list --created-after 2024-01-01` | ❌ | ✅ Search API `created:>date` | ✅ | **Gap.** Adicionar `created_after`, `created_before`, `updated_after`, `updated_before` ao `list`. Filtrável no cache |
| 6.9 | Empty/null checks | `bd list --empty-description --no-assignee` | ❌ | ⚠️ Search API `no:assignee` | ✅ | **Gap menor.** Filtros `no_assignee`, `no_labels` no `list`. Trivial sobre cache |
| 6.10 | Priority ranges | `bd list --priority-min 0 --priority-max 1` | ⚠️ `list --priority` (valor exacto) | ⚠️ Field filter | ✅ | **Gap.** Converter `priority` filter para aceitar range. `priority_min`, `priority_max` |

---

## 7. Advanced Operations

| # | Beads Feature | Beads Command | Unblock Equivalente | GitHub Nativo | MCP Viável | Notas |
|---|---|---|---|---|---|---|
| 7.1 | Cleanup closed issues | `bd admin cleanup --older-than 30` | ❌ | ⚠️ Close ≠ delete em GitHub | ⚠️ | GitHub não apaga issues. Closed issues ficam no repo. Podemos `archive` via Projects V2 (remover do board). Não equivalente a delete |
| 7.2 | Duplicate detection | `bd duplicates --auto-merge` | ❌ | ❌ | ✅ | **Gap interessante.** MCP tool `duplicates` que usa title similarity + body overlap. Agent pode fazer merge manual. Auto-merge é arriscado |
| 7.3 | Merge issues | `bd merge src --into target` | ❌ | ❌ | ✅ | **Gap.** PRD já lista `merge` tool como future enhancement. MCP fecha source, move deps/labels/comments para target |
| 7.4 | Compaction (memory decay) | `bd admin compact --auto` | N/A | N/A | N/A | Beads-specific. Reduz tamanho de issues velhas em SQLite. GitHub não tem este problema — issues são armazenadas no servidor |
| 7.5 | Rename prefix | `bd rename-prefix kw-` | N/A | N/A | N/A | Beads-specific. Issues GitHub usam números, não prefixos |
| 7.6 | Restore de git history | `bd restore <id>` | N/A | ✅ GitHub mantém histórico nativo | N/A | GitHub é o source of truth — edits têm audit trail nativo |

---

## 8. Molecular Chemistry (Templates)

| # | Beads Feature | Beads Command | Unblock Equivalente | GitHub Nativo | MCP Viável | Notas |
|---|---|---|---|---|---|---|
| 8.1 | Formula/proto list | `bd formula list` | ❌ | ✅ Issue Templates | ⚠️ | GitHub tem Issue Templates (`.github/ISSUE_TEMPLATE/*.md`). Não têm deps/hierarchy. MCP poderia ter `template_pour` que cria issue set a partir de template com variáveis |
| 8.2 | Pour (instanciar template) | `bd mol pour <proto> --var key=val` | ❌ | ⚠️ Issue Templates (sem vars) | ✅ | **Gap interessante.** Templates parametrizadas com variáveis → N issues com deps. Similar a 1.4 (create from plan). Uma tool `plan_execute` que aceita template + vars e cria o grafo |
| 8.3 | Wisp (ephemeral issues) | `bd mol wisp` | ❌ | ❌ | ⚠️ | Conceito Beads: issues temporárias que não são exportadas. GitHub não tem conceito de "ephemeral". Poderíamos usar label `ephemeral` mas seriam issues reais. Não recomendado — viola P1 "GitHub stores" |
| 8.4 | Bond (combine work) | `bd mol bond A B --type sequential` | ❌ | ⚠️ Blocking = sequential | ⚠️ | `blocks` nativo = sequential. Parallel/conditional não existem. MCP poderia interpretar `bond --parallel` como "sem dep entre A e B, ambos filhos do mesmo parent" |
| 8.5 | Squash wisp → digest | `bd mol squash` | N/A | N/A | N/A | Beads-specific lifecycle |
| 8.6 | Burn wisp | `bd mol burn` | N/A | N/A | N/A | Beads-specific lifecycle |

---

## 9. Database / Sync

| # | Beads Feature | Beads Command | Unblock Equivalente | GitHub Nativo | MCP Viável | Notas |
|---|---|---|---|---|---|---|
| 9.1 | Import JSONL | `bd import -i issues.jsonl` | N/A | ✅ GitHub é o source | N/A | Não aplicável. GitHub É a base de dados. Beads precisa de import/export porque usa SQLite local |
| 9.2 | Export | `bd sync` | N/A | ✅ GitHub API = export | N/A | `prime` + cache serve o mesmo propósito conceptual |
| 9.3 | Migration | `bd migrate` | ✅ `setup --migrate` | ✅ REST | — | `setup --migrate` adiciona issues existentes ao Project |
| 9.4 | Daemon management | `bd daemons list/stop/restart` | N/A | N/A | N/A | Beads roda daemon local. Unblock é stateless MCP server. Lifecycle gerido pelo host (Claude Code) |
| 9.5 | Sync (git push/pull) | `bd sync` | N/A | ✅ GitHub = remote by design | N/A | Não aplicável. GitHub é always-synced |

---

## 10. Operational / Health

| # | Beads Feature | Beads Command | Unblock Equivalente | GitHub Nativo | MCP Viável | Notas |
|---|---|---|---|---|---|---|
| 10.1 | Info / status | `bd info --json` | ✅ `doctor` | ✅ API health | — | `doctor` cobre: project linked, fields valid, issues in project, graph consistency, rate limits |
| 10.2 | Stats | Implícito | ✅ `stats` | ⚠️ Projects Insights | — | `stats` com breakdown por status, priority, blocked/ready count, cycles, agents |
| 10.3 | Staleness control | `bd --allow-stale` | ✅ Cache `stale: true` flag | N/A | — | Unblock serve stale cache automaticamente com flag. Princípio P3: "fail open for reads" |
| 10.4 | Sandbox mode | `bd --sandbox` | N/A | N/A | N/A | Beads-specific. MCP server não precisa — é stateless por design |
| 10.5 | Custom actor | `bd --actor alice` | ✅ `claim --agent`, `UNBLOCK_AGENT` | N/A | — | Paridade |

---

## 11. Editor Integration

| # | Beads Feature | Beads Command | Unblock Equivalente | GitHub Nativo | MCP Viável | Notas |
|---|---|---|---|---|---|---|
| 11.1 | Setup Claude Code | `bd setup claude` | ✅ Plugin `.mcp.json` + slash commands + skills | N/A | — | Unblock tem plugin nativo Claude Code com hooks e workflow skill |
| 11.2 | Setup Cursor | `bd setup cursor` | ❌ | N/A | ✅ | **Gap.** Gerar `.cursor/rules/unblock.mdc` com workflow instructions. Trivial — template estático |
| 11.3 | Setup Aider | `bd setup aider` | ❌ | N/A | ✅ | **Gap.** Gerar `.aider.conf.yml`. Trivial |
| 11.4 | Setup Factory | `bd setup factory` | ❌ | N/A | ✅ | **Gap.** Gerar/actualizar `AGENTS.md`. Trivial |
| 11.5 | Check/remove hooks | `bd setup claude --check/--remove` | ❌ | N/A | ✅ | **Gap menor.** Adicionar `setup --check`, `setup --remove` |

---

## 12. Issue Types

| Beads Type | Unblock Equivalente | GitHub Nativo | Notas |
|---|---|---|---|
| `bug` | ✅ Issue Type `bug` | ✅ Issue Types (GA 2025) | Paridade |
| `feature` | ✅ Issue Type `feature` | ✅ Issue Types | Paridade |
| `task` | ✅ Issue Type `task` | ✅ Issue Types | Paridade |
| `epic` | ✅ Milestones + Sub-Issues | ✅ Sub-Issues hierarchy | Milestones como agrupador, Sub-Issues para hierarquia |
| `chore` | ⚠️ Via label | ⚠️ Custom issue type | Podemos adicionar como Issue Type |
| `tombstone` | N/A | N/A | Beads-specific. GitHub closed = closed |
| `pinned` | ❌ | ⚠️ Pinned issues no repo | ✅ Label `pinned` + exclusão do ready set |

---

## 13. Issue Statuses

| Beads Status | Unblock Equivalente | GitHub Nativo | Notas |
|---|---|---|---|
| `open` | ✅ Status: `open` | ✅ Projects V2 field | Paridade |
| `in_progress` | ✅ Status: `in_progress` | ✅ Projects V2 field | Paridade |
| `blocked` | ✅ Status: `blocked` | ✅ Projects V2 field | Paridade |
| `deferred` | ✅ Status: `deferred` | ✅ Projects V2 field | Paridade |
| `closed` | ✅ Status: `closed` | ✅ Projects V2 field | Paridade |
| `tombstone` | N/A | N/A | Beads-specific para supressão de resurrect |
| `pinned` | ❌ | ❌ | Beads-specific para hooks/anchors. Ver 12 acima |

---

## 14. Dependency Types

| Beads Type | Unblock Equivalente | GitHub Nativo | MCP Viável | Notas |
|---|---|---|---|---|
| `blocks` | ✅ `depends` / `blocked_by` | ✅ Blocking API nativo | — | Paridade total. Único tipo que afecta ready queue (ambos os sistemas) |
| `related` | ❌ | ⚠️ Issue mentions (#N) | ⚠️ | Informal via menções. Sem query API formal. Podemos enforçar via comment pattern |
| `parent-child` | ✅ Sub-Issues | ✅ Sub-Issues API | — | Paridade total |
| `discovered-from` | ⚠️ `blocked_by` + comment | ⚠️ Blocking + comment | ✅ | Pattern semântico. MCP pode aceitar `discovered_from` no `create` que cria comment "Discovered while working on #X" sem criar dep de blocking |

---

## Resumo de Gaps Prioritários

### Alta prioridade (melhoram significativamente o workflow de agents)

| # | Feature | Esforço | Caminho |
|---|---|---|---|
| G1 | **Batch operations** — `ids: [1,2,3]` em `update`, `close`, `reopen`, `show` | Baixo | MCP: iterar internamente, batch GraphQL mutations |
| G2 | **Stale issues** — `stale` tool | Baixo | MCP: filtro `updated_at < N days` sobre cache |
| G3 | **Dep tree** — árvore recursiva de deps | Baixo | MCP: graph engine já tem dados, expor traversal |
| G4 | **Date range filters** no `list` | Baixo | MCP: filtros adicionais sobre cache |
| G5 | **Priority range** no `list` | Trivial | MCP: `priority_min` / `priority_max` |

### Média prioridade (diferenciam para equipas)

| # | Feature | Esforço | Caminho |
|---|---|---|---|
| G6 | **Create from plan** — `.md` → N issues com deps | Médio | MCP: parser markdown → batch create |
| G7 | **Merge/duplicates** | Médio | MCP: já listado como future enhancement no PRD |
| G8 | **Label OR filter** no `list` | Trivial | MCP: `label_any` com lógica OR |
| G9 | **`discovered_from`** como dep type semântico | Baixo | MCP: comment pattern + label, sem blocking edge |
| G10 | **Setup multi-editor** — Cursor, Aider, Factory | Baixo | MCP: `setup --editor cursor\|aider\|factory` gera config files |

### Baixa prioridade / Não aplicável

| # | Feature | Razão |
|---|---|---|
| — | Molecular chemistry (wisp, bond, squash, burn) | Beads-specific lifecycle. GitHub não tem ephemeral issues. Templates parametrizadas (G6) cobrem o caso útil |
| — | Import/export/sync/daemon | Não aplicável. GitHub é o source of truth. Zero local storage |
| — | Compaction/rename-prefix/restore | SQLite-specific. GitHub mantém tudo server-side |
| — | Sandbox mode | Unblock é stateless MCP server, sem daemon |
| — | Explicit IDs | GitHub numera sequencialmente |
| — | Tombstone status | GitHub closed + archived = suficiente |

---

## Conclusão Arquitectural

O Unblock cobre **~75% das features do Beads** que fazem sentido num contexto GitHub-native. Os ~25% restantes dividem-se em:

- **Gaps reais e implementáveis (G1-G10):** maioritariamente filtros avançados e batch operations. Esforço total estimado: 2-3 dias de desenvolvimento.
- **Features Beads-specific:** molecular chemistry, daemon, import/export, compaction — são consequência do Beads usar SQLite local. Não se aplicam quando GitHub é o backend.

A maior vantagem arquitectural do Unblock sobre o Beads é que **não tem infra local**. Beads precisa de daemon, sync, import/export, migration, orphan handling, staleness detection — tudo porque o SQLite local pode divergir. O Unblock não tem nenhum destes problemas porque o GitHub API é always-consistent e o cache é ephemeral.

A maior vantagem do Beads sobre o Unblock é o **sistema de templates parametrizadas** (molecules). O gap G6 (create from plan) é o mais impactante para fechar esta diferença.
