@echo off

echo "Build en cours ..."
cargo build --target armv7-unknown-linux-gnueabihf && (

echo "Envoi du build ..."
scp -B target/armv7-unknown-linux-gnueabihf/debug/voiturerc master@192.168.1.8:voiturerc && (
echo "Changement de permission ..."
ssh master@192.168.1.8 "chmod +x voiturerc" && (
echo "Terminée."
) || (
    echo "Echec lors de la modification des permissions"
)
) || (
    echo "Echec lors de l'envoi du build"
)
) || (
    echo "Echec lors de la compilation"
)
