# Albion Stats

[![Release](https://img.shields.io/github/v/release/ZainadosRepo/AlbionStatsTools)](https://github.com/ZainadosRepo/AlbionStatsTools/releases/latest)
[![CI](https://github.com/ZainadosRepo/AlbionStatsTools/actions/workflows/ci.yml/badge.svg)](https://github.com/ZainadosRepo/AlbionStatsTools/actions/workflows/ci.yml)

Application de bureau native en **Rust + Tauri 2** dédiée au suivi en temps réel des statistiques de combat, de session, de récolte, de donjons et d'économie pour **Albion Online**.

L'application observe passivement les paquets réseau **Photon UDP** non chiffrés sur les ports `5055-5060` et `4530-4540` (via **Npcap** en priorité sous Windows, avec repli sur les sockets bruts multi-interfaces IPv4 `SIO_RCVALL` / IPv6 `IPPROTO_UDP`), décode les flux binaires **Protocol 16 & Protocol 18** à la volée, et met à disposition un compteur temps réel, un inspecteur de sessions complet ainsi qu'un calculateur économique relié à l'API publique **Albion Data Project**.

## Télécharger

Dernière version : **[Releases › Latest](https://github.com/ZainadosRepo/AlbionStatsTools/releases/latest)**

| Système | Fichier |
| --- | --- |
| Windows 10 / 11 (64 bits) | `Albion.Stats_<version>_x64-setup.exe` |
| macOS (Intel et Apple Silicon) | `Albion.Stats_<version>_universal.dmg` |

### Windows

1. Lancer `Albion.Stats_<version>_x64-setup.exe`.
2. L'installeur n'est pas signé : si Windows SmartScreen s'affiche, cliquer sur **Informations complémentaires**, puis **Exécuter quand même**.
3. Suivre l'installeur ; Albion Stats apparaît dans le menu Démarrer.

### macOS

1. Ouvrir le `.dmg` et glisser **Albion Stats** dans **Applications**.
2. L'appli n'est pas signée : au premier lancement, macOS la bloque. Ouvrir **Réglages Système › Confidentialité et sécurité**, puis cliquer sur **Ouvrir quand même** en bas de la page.

### Mettre à jour

Au lancement, l'appli regarde sur GitHub si une version plus récente est publiée et propose de l'installer : **Installer** la télécharge, vérifie sa signature et redémarre l'appli ; **Plus tard** la reproposera au prochain lancement. Hors ligne, rien ne s'affiche. Les profils, sessions archivées et réglages sont conservés.

---

## ✨ Fonctionnalités

- **⚔ Compteur de Combat (Damage / Heal / Overheal / Subis)**
  - Calcul en temps réel des **Dégâts**, **DPS**, **Soins**, **HPS**, **Overheal (%)** et **Dégâts subis** pour l'ensemble du groupe.
  - **Ventilation détaillée par sort** en français (icônes Albion avec cache persistant, nombre de ticks, dégâts, soins, overheal, part relative en %).
  - **Inférence automatique de l'arme** dès le premier sort lancé (`spells.xml` / `items.xml` / `<craftingspelllist>`), même avant l'inspection complète de l'équipement d'un allié.
  - **Snapshots de combat** et copie rapide en 1 clic dans le presse-papiers pour le chat de groupe (format compact ou détaillé).
- **📈 Inspecteur de Sessions & Continuité inter-zones**
  - Suivi unifié de la **Session en direct** et des **Sessions archivées** avec sous-vues dédiées : *Vue d'ensemble*, *Zones traversées*, *Récolte*, *Combat & Sorts*, *Donjons*, *Loot & Échanges* et *Factions*.
  - Suivi des taux horaires (**Renommée/h**, **Silver/h**, **ReSpec/h**, **Puissance/h**, **Faveur/h**), taxes de guilde/zone, coûts de réparation, et continuité transparente du joueur via `Guid16` lors des changements de map/instance.
  - Points et jetons de **Guerre de Factions** par cité royale (`Martlock`, `Lymhurst`, `Bridgewatch`, `Fort Sterling`, `Thetford`, `Caerleon`, `Brecilien`).
- **🏰 Suivi des Donjons, Brumes (Mists), Hellgates & Corrompus**
  - Détection automatique des entrées d'instances, chronomètre, renommée et argent par instance, coffres ouverts et feux follets en cage libérés.
- **⛏ Journal de Récolte (Gathering)**
  - Comptabilisation par filon (`Bois`, `Minerai`, `Fibre`, `Peau`, `Pierre`, `Poisson`) avec séparation du rendement de base, du **bonus de récolteur** et du **bonus Premium**, ainsi que l'estimation en Silver.
- **🎒 Journal de Butin & Équipement du Groupe**
  - Inspection des 10 emplacements d'équipement du joueur local et des membres de son groupe uniquement, avec estimation de la valeur du build.
  - Journal de butin du joueur et du groupe (Silver décodé au format `FixPoint` et objets) avec fenêtre de **déduplication de 2 secondes** et export **CSV**.
- **⚖ Marché, Courriers, Catalogue `ao-bin-dumps` & Calculateur d'Artisanat (RRR)**
  - Synchronisation automatique ou manuelle du catalogue complet depuis **ao-bin-dumps** (`items.json`, `world.json`, `spells.xml`, `items.xml`).
  - Calculateur de rendement de retour des ressources (`RRR = 1 - 1 / (1 + bonus / 100)`), frais de station par 100 points de nutrition, profit des journaux, taxes du marché (avec/sans Premium, vente directe ou ordre de vente) et marge nette par lot.
  - Interrogation directe des serveurs **Albion Data Project** (`Europe`, `Amériques`, `Asie`) et détection des écarts d'arbitrage entre cités.
- **🪟 Fenêtre Séparée & Conformité SBI (Sandbox Interactive)**
  - **Lecture passive externe uniquement** : aucune injection mémoire/DLL ni modification des fichiers du client de jeu.
  - **Aucun overlay intrusif en jeu** : l'application s'exécute dans une fenêtre séparée opaque standard (idéale sur un second écran ou en tâche de fond, avec mode d'affichage **Compact** adaptatif), sans superposition *Always on Top* ni souris traversante.
  - **Limitation stricte au groupe** : seules vos propres statistiques et celles des membres de votre groupe sont mesurées et affichées (aucune donnée ni inspection sur les joueurs tiers ou ennemis, aucun radar d'entités).
  - **Zéro automatisation** : aucune macro, action automatique ni alerte tactique en combat.

---

## 🚀 Développement & Vérification

### Prérequis
- **Rust** (`stable`, édition 2021)
- **Node.js** (18+ pour les tests unitaires de l'interface ES modules)
- Sous Windows : **WebView2** (inclus par défaut sur Windows 10/11) et **Npcap** recommandé pour la capture sans élévation

### Commandes utiles

```bash
# Formater et vérifier le code Rust
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings

# Exécuter la suite de tests Rust (parseur Photon/Protocol16/Protocol18, moteur de combat, artisanat, persistance)
cargo test --workspace --locked

# Exécuter la suite de tests unitaires JavaScript (UI)
node --test "tests/ui/*.test.js"
```

### Capture réseau sous Windows
1. **Méthode recommandée (Npcap)** : installez [Npcap](https://npcap.com/) avec l'option *Install Npcap in WinPcap API-compatible Mode* — la capture fonctionne automatiquement sans exécuter l'application en Administrateur.
2. **Sans Npcap** : cliquez sur **Activer Capture Admin** dans la barre supérieure pour relancer avec élévation UAC et activer l'écoute passive multi-interfaces via sockets bruts (`SIO_RCVALL` / `IPPROTO_UDP`).
3. **Mode veille** : sans Npcap ni droits Administrateur, l'application écoute en veille sur le relais UDP local `127.0.0.1:50560`.
