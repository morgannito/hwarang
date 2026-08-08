# ADR-0001 — Réécriture intégrale, aucun code propriétaire

- **Statut** : accepté
- **Date** : 2026-08-08

## Contexte

L'écosystème des serveurs privés Metin2 se construit sur des serverfiles et des
sources fuités de Ymir Entertainment / GameForge. Repartir de cette base est la
voie rapide : le jeu tourne en une soirée.

Trois contraintes s'y attachent :

1. **Juridique** — le code est propriétaire. Le projet ne peut être ni publié
   sous licence libre, ni monétisé, ni rendu visible sans exposition au DMCA.
2. **Sécurité** — les repacks publics contiennent fréquemment des portes
   dérobées (accès GM, exfiltration de base). L'audit d'un binaire C++ fuité
   coûte plus cher que la réécriture des sous-systèmes concernés.
3. **Technique** — Direct3D 8, Granny 3D (SDK fermé, licence par plateforme),
   C++ 2005, binaires x86 32 bits. Aucun de ces plafonds ne se lève par du
   travail incrémental.

## Décision

Réécriture intégrale. Aucune ligne de code, aucun asset, aucun fichier de
données issu du jeu d'origine n'entre dans le dépôt.

Ce qui reste autorisé : s'inspirer des **mécaniques de jeu** telles
qu'observables en jouant. Une règle de gameplay n'est pas protégeable ; son
implémentation l'est.

Ce qui est interdit : décompiler pour transcrire, importer `item_proto` ou
`mob_proto`, reprendre les formules exactes issues des sources fuitées,
extraire des modèles, textures, sons ou cartes.

## Conséquences

**Positives** — projet publiable sous AGPL-3.0 ; contributions externes
possibles ; portage multi-plateforme non contraint par Granny ni par le 32 bits ;
aucune surface d'attaque héritée.

**Négatives** — pas de contenu prêt à l'emploi ; l'art devient le poste de coût
dominant ; le premier jalon jouable est très loin d'une soirée.

**Discipline** — toute pull request introduisant un fichier d'origine incertaine
est refusée. En cas de doute sur la provenance d'un asset, il ne rentre pas.
