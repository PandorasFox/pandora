# Maintainer: hecate <sys@hecate.space>
pkgname=pandora-git
pkgver=0.0.1
pkgrel=1
pkgdesc="a parallax-scrolling wallpaper and lockscreen daemon for wayland compositors"
arch=('i686' 'x86_64' 'armv6h' 'armv7h')
url="https://github.com/pandorasfox/pandora"
# todo validate
license=('GPL-2.0')
# todo
depends=()
makedepends=(cargo)
checkdepends=()
optdepends=(niri)
provides=(pandora)
conflicts=()
replaces=()
backup=()
options=()
install=
changelog=
source=()
noextract=()
sha256sums=()
validpgpkeys=()

prepare() {
    export RUSTUP_TOOLCHAIN=stable
    cargo fetch --locked --target "$(rustc -vV | sed -n 's/host: //p')"
}

build() {
	export RUSTUP_TOOLCHAIN=stable
    export CARGO_TARGET_DIR=target
    cargo build --frozen --release --all-features
}

check() {
	export RUSTUP_TOOLCHAIN=stable
    cargo test --frozen --all-features
}

package() {
	install -Dm0755 -t "$pkgdir/usr/bin/" "target/release/pandora"
}
