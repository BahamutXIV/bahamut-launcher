# make -C <extracted dir> install runs install.sh from that directory;
# DESTDIR stages a package root. PKGDIR is not taken from the environment.
PREFIX ?= /usr/local
PKGDIR = $(PREFIX)/lib/bahamut-launcher
DESTDIR ?=

.PHONY: install uninstall

install:
	./install.sh --prefix "$(PREFIX)" --pkgdir "$(PKGDIR)" --destdir "$(DESTDIR)"

uninstall:
	./install.sh --uninstall --prefix "$(PREFIX)" --pkgdir "$(PKGDIR)" --destdir "$(DESTDIR)"
