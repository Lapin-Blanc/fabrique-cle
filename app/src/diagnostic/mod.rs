//! Onglet « Diagnostic » : outil purement informatif, indépendant de la fabrication.
//! Il identifie la clé (ce qu'elle et le système déclarent), mesure ses performances en lecture
//! seule (étape 1) ou de façon complète en effaçant la clé (étape 2), et explique chaque facette technique au regard de l'usage de Fabrique-Clé : faire
//! tourner un système depuis la clé.
//! Seule l'analyse complète écrit sur la clé, après confirmation.

pub mod analyse;
pub mod interpretation;
pub mod materiel;
pub mod mesures;
pub mod textes;
