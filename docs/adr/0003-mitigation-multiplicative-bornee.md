# ADR-0003 — Mitigation multiplicative bornée

- **Statut** : accepté
- **Date** : 2026-08-08

## Contexte

Le calcul de dégâts de Metin2 empile les réductions de façon **additive** :
armure, résistances élémentaires, résistances par type d'arme et bonus divers
s'additionnent en pourcentage. Passé un certain niveau d'équipement, la somme
approche puis dépasse 100 % et les dégâts tombent à zéro.

Conséquences observées sur les serveurs privés : personnages intuables en PvP,
et une accumulation de rustines au cas par cas (plafonds arbitraires par source,
exclusions codées en dur) qui rend l'équilibrage impossible à raisonner.

La cause n'est pas un mauvais réglage : c'est le modèle mathématique. Une somme
de pourcentages n'est pas bornée.

## Décision

Deux réductions successives, chacune bornée par construction.

**Armure — rendement décroissant asymptotique :**

```
K       = ARMOR_SCALING × niveau_défenseur
dégâts  = puissance × K / (armure + K)
```

Le ratio `K / (armure + K)` appartient à `]0, 1]` quelle que soit l'armure, y
compris `u32::MAX`. À `armure = K`, la réduction vaut exactement 50 %. Indexer
`K` sur le niveau du défenseur évite qu'une armure de bas niveau conserve son
efficacité en fin de progression.

**Résistances — plafond dans le type :**

`Resistance` encode des pour mille et écrête à `MAX_PERMILLE = 900` (90 %) dans
son constructeur. Le plafond est une propriété du type, pas une vérification
dans la formule : il n'existe aucun chemin de code capable de construire une
résistance supérieure, quelles que soient les sources additionnées en amont.

**Plancher :** `MIN_DAMAGE = 1`. Une attaque qui touche fait toujours quelque
chose.

## Propriétés garanties

Vérifiées par les tests de `hwarang-domain::combat` :

- les dégâts sont toujours `>= MIN_DAMAGE`, y compris à armure `u32::MAX` et
  résistance maximale ;
- la mitigation est monotone décroissante en fonction de l'armure ;
- l'armure a un rendement strictement décroissant ;
- une armure fixe perd de son efficacité à mesure que le niveau monte ;
- aucun débordement sur les valeurs extrêmes.

## Conséquences

Le calibrage se réduit à une seule constante, `ARMOR_SCALING`, lisible comme
« l'armure qui divise les dégâts par deux, par niveau ». Il n'y a plus de
plafond arbitraire à ajouter par source de mitigation.

`resolve_attack` reste une fonction pure et déterministe. La variance
(critiques, esquive) relève d'une couche supérieure, ce qui garde le calcul
rejouable à l'identique pour auditer un combat contesté.

Le tuning diffère de l'original : les valeurs d'armure des objets ne sont pas
transposables telles quelles, et ne doivent pas l'être (ADR-0001).
