//! Lecture d'un fichier `.bmap` (produit par `bmaptool create`) : liste des blocs utilisés de
//! l'image. Seuls ces blocs sont écrits sur la clé (l'image de 10 Gio n'en occupe qu'une partie),
//! et chacun est vérifié par relecture grâce à sa somme SHA-256.

#[derive(Clone, Debug, PartialEq)]
pub struct Plage {
    /// Premier bloc (inclus).
    pub debut: u64,
    /// Dernier bloc (inclus).
    pub fin: u64,
    /// SHA-256 (hexadécimal) du contenu de la plage, si le fichier est au format 2.x.
    pub sha256: Option<String>,
}

#[derive(Clone, Debug)]
pub struct Bmap {
    pub taille_image: u64,
    pub taille_bloc: u64,
    pub plages: Vec<Plage>,
}

impl Bmap {
    /// Position et longueur (en octets) d'une plage, bornées à la taille de l'image.
    pub fn octets(&self, p: &Plage) -> (u64, u64) {
        let debut = p.debut * self.taille_bloc;
        let fin = ((p.fin + 1) * self.taille_bloc).min(self.taille_image);
        (debut, fin.saturating_sub(debut))
    }

    /// Nombre total d'octets à écrire.
    pub fn octets_utiles(&self) -> u64 {
        self.plages.iter().map(|p| self.octets(p).1).sum()
    }
}

/// Texte entre `<balise>` et `</balise>`, espaces retirés.
fn valeur<'a>(xml: &'a str, balise: &str) -> Option<&'a str> {
    let ouvrante = format!("<{balise}>");
    let i = xml.find(&ouvrante)? + ouvrante.len();
    let j = xml[i..].find(&format!("</{balise}>"))? + i;
    Some(xml[i..j].trim())
}

fn nombre(xml: &str, balise: &str) -> Result<u64, String> {
    valeur(xml, balise)
        .ok_or_else(|| format!("bmap : <{balise}> absent"))?
        .parse()
        .map_err(|_| format!("bmap : <{balise}> invalide"))
}

/// Retire les commentaires `<!-- … -->` (bmaptool y cite des balises comme `<BlocksCount>`).
fn sans_commentaires(xml: &str) -> String {
    let mut res = String::with_capacity(xml.len());
    let mut reste = xml;
    while let Some(i) = reste.find("<!--") {
        res.push_str(&reste[..i]);
        reste = reste[i..].find("-->").map_or("", |j| &reste[i + j + 3..]);
    }
    res.push_str(reste);
    res
}

pub fn analyser(xml: &str) -> Result<Bmap, String> {
    let xml = &sans_commentaires(xml);
    let taille_image = nombre(xml, "ImageSize")?;
    let taille_bloc = nombre(xml, "BlockSize")?;
    if taille_bloc == 0 || taille_bloc % 512 != 0 {
        return Err("bmap : taille de bloc invalide".into());
    }
    let sha256 = valeur(xml, "ChecksumType") == Some("sha256");

    let mut plages = Vec::new();
    let mut reste: &str = xml;
    while let Some(i) = reste.find("<Range") {
        reste = &reste[i + "<Range".len()..];
        let fin_attr = reste.find('>').ok_or("bmap : <Range> mal formé")?;
        let attributs = &reste[..fin_attr];
        let fin_txt = reste.find("</Range>").ok_or("bmap : </Range> manquant")?;
        let texte = reste[fin_attr + 1..fin_txt].trim();
        reste = &reste[fin_txt..];

        let somme = attributs
            .split("chksum=\"")
            .nth(1)
            .and_then(|s| s.split('"').next())
            .map(|s| s.to_ascii_lowercase());
        let (a, b) = match texte.split_once('-') {
            Some((a, b)) => (a.trim(), b.trim()),
            None => (texte, texte),
        };
        let (debut, fin): (u64, u64) = (
            a.parse()
                .map_err(|_| format!("bmap : plage invalide « {texte} »"))?,
            b.parse()
                .map_err(|_| format!("bmap : plage invalide « {texte} »"))?,
        );
        if fin < debut || debut * taille_bloc >= taille_image {
            return Err(format!("bmap : plage hors de l'image « {texte} »"));
        }
        plages.push(Plage {
            debut,
            fin,
            sha256: if sha256 { somme } else { None },
        });
    }
    plages.sort_by_key(|p| p.debut);
    if plages.windows(2).any(|w| w[1].debut <= w[0].fin) {
        return Err("bmap : plages qui se chevauchent".into());
    }
    Ok(Bmap {
        taille_image,
        taille_bloc,
        plages,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXEMPLE: &str = r#"<?xml version="1.0" ?>
<!-- Commentaire de bmaptool : <ImageSize> 1 </ImageSize> <Range> 99 </Range> -->
<bmap version="2.0">
    <ImageSize> 40960 </ImageSize>
    <BlockSize> 4096 </BlockSize>
    <BlocksCount> 10 </BlocksCount>
    <MappedBlocksCount> 4 </MappedBlocksCount>
    <ChecksumType> sha256 </ChecksumType>
    <BmapFileChecksum> 0000 </BmapFileChecksum>
    <BlockMap>
        <Range chksum="ABCD"> 7-9 </Range>
        <Range chksum="ef01"> 0 </Range>
    </BlockMap>
</bmap>"#;

    #[test]
    fn lecture() {
        let b = analyser(EXEMPLE).unwrap();
        assert_eq!(b.taille_image, 40960);
        assert_eq!(b.taille_bloc, 4096);
        assert_eq!(
            b.plages,
            vec![
                Plage {
                    debut: 0,
                    fin: 0,
                    sha256: Some("ef01".into())
                },
                Plage {
                    debut: 7,
                    fin: 9,
                    sha256: Some("abcd".into())
                },
            ]
        );
        assert_eq!(b.octets(&b.plages[1]), (28672, 12288));
        assert_eq!(b.octets_utiles(), 16384);
    }

    #[test]
    fn refus() {
        assert!(analyser("<bmap></bmap>").is_err());
        let hors = EXEMPLE.replace("7-9", "12-13");
        assert!(analyser(&hors).is_err());
        let chevauche = EXEMPLE.replace("> 0 <", "> 8 <");
        assert!(analyser(&chevauche).is_err());
    }
}
