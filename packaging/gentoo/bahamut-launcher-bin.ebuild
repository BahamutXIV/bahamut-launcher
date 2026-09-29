# Copyright 2026 BahamutXIV
# Distributed under the terms of the MIT License

EAPI=8

inherit desktop xdg

DESCRIPTION="Launcher for the BahamutXIV Final Fantasy XIV 1.23b server"
HOMEPAGE="https://github.com/BahamutXIV/bahamut-launcher"
SRC_URI="https://github.com/BahamutXIV/bahamut-launcher/releases/download/v${PV}/bahamut-launcher-v${PV}-linux-x86_64.tar.gz"
S="${WORKDIR}/bahamut-launcher"

LICENSE="MIT BSD-2 OFL-1.1"
SLOT="0"
KEYWORDS="-* ~amd64"
RESTRICT="mirror strip"

RDEPEND="
	net-libs/webkit-gtk:4.1
	x11-libs/gtk+:3
	virtual/wine
"

QA_PREBUILT="opt/bahamut-launcher/bahamut-launcher"

src_compile() { :; }

src_install() {
	insinto /opt/bahamut-launcher
	doins -r .bahamut-launcher-package bahamut-loader.exe bahamut.dll \
		plugins addons scripts licenses LICENSE.md README.md

	exeinto /opt/bahamut-launcher
	doexe bahamut-launcher
	dosym -r /opt/bahamut-launcher/bahamut-launcher /usr/bin/bahamut-launcher

	domenu share/applications/bahamut-launcher.desktop

	local s
	for s in 48 128 256; do
		newicon -s ${s} share/icons/hicolor/${s}x${s}/apps/bahamut-launcher.png bahamut-launcher.png
	done
}

pkg_postinst() {
	xdg_pkg_postinst
	elog "The launcher needs a Wine build with 32-bit support (abi_x86_32 or wow64)."
	elog "Launcher state lives in ~/.bahamut-launcher."
}
