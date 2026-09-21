# Lyra — no-xcodeproj build. Rust core → staticlib → swiftc → .app bundle → codesign.
# For release we'd move to XcodeGen for Sparkle's nested-signing (see BLUEPRINT § packaging).

CARGO    := $(if $(shell command -v mise 2>/dev/null),mise exec -- cargo,cargo)
PROFILE  := debug
SWIFTC   := xcrun swiftc
APP      := .build/Lyra.app
APPBIN   := $(APP)/Contents/MacOS/Lyra
LIBDIR   := app/.libs
SWIFT_SRC:= $(wildcard app/Sources/LyraApp/*.swift app/Sources/LyraApp/*/*.swift)
UNAME_S  := $(shell uname -s)

.PHONY: all rust app sign run clean check lyrad gui e2e darwin-only

all: app

check:
	$(CARGO) check --workspace

# The SwiftUI shell is macOS-only; on Linux the UI is lyra-gui (GTK4 +
# libadwaita, needs libgtk-4-dev + libadwaita-1-dev) and the supported
# headless host is lyrad — both are driven by the `lyra` CLI over IPC.
darwin-only:
	@test "$(UNAME_S)" = "Darwin" || { echo "make: the SwiftUI app is macOS-only — use 'make gui' or 'make lyrad' on Linux"; exit 1; }

# Headless host — works on Linux and macOS.
lyrad:
	$(CARGO) build -p lyra-ffi --bin lyrad $(if $(filter release,$(PROFILE)),--release,)

# Linux-native UI (GTK4 + libadwaita). Builds on macOS too with
# `brew install gtk4 libadwaita`, but the supported mac UI is SwiftUI.
gui:
	$(CARGO) build -p lyra-ui --bin lyra-gui $(if $(filter release,$(PROFILE)),--release,)

# IPC-driven E2E: same asserts on both OSes — macOS runs the built .app,
# Linux runs lyra-gui under Xvfb and lyrad headless (see scripts/e2e.sh).
e2e:
	$(CARGO) build --bins -p lyra-cli -p lyra-ffi $(if $(filter release,$(PROFILE)),--release,)
	@if [ "$(UNAME_S)" = "Darwin" ]; then \
		$(MAKE) app PROFILE=$(PROFILE); \
		LYRA_BIN=$$PWD/target/$(PROFILE)/lyra LYRA_E2E_APP=$$PWD/$(APP) ./scripts/e2e.sh; \
	else \
		$(CARGO) build -p lyra-ui --bin lyra-gui $(if $(filter release,$(PROFILE)),--release,); \
		LYRA_BIN=$$PWD/target/$(PROFILE)/lyra LYRA_E2E_HOST=$$PWD/target/$(PROFILE)/lyra-gui ./scripts/e2e.sh; \
		LYRA_BIN=$$PWD/target/$(PROFILE)/lyra LYRA_E2E_HOST=$$PWD/target/$(PROFILE)/lyrad ./scripts/e2e.sh; \
	fi

rust:
	$(CARGO) build -p lyra-ffi $(if $(filter release,$(PROFILE)),--release,)
	@mkdir -p $(LIBDIR)
	@cp target/$(PROFILE)/liblyra_ffi.a $(LIBDIR)/

app: darwin-only rust $(APPBIN) $(APP)/Contents/Info.plist $(APP)/Contents/Resources/AppIcon.icns $(APP)/Contents/Resources/Assets.car sign

$(APP)/Contents/Resources/AppIcon.icns: app/Resources/AppIcon.icns
	@mkdir -p $(APP)/Contents/Resources
	@cp app/Resources/AppIcon.icns $(APP)/Contents/Resources/

# Regenerate the icon: swift scripts/make_icon.swift → iconset → icns.
app/Resources/AppIcon.icns: scripts/make_icon.swift
	swift scripts/make_icon.swift .build/icon_1024.png
	@mkdir -p .build/lyra.iconset
	@for s in 16 32 64 128 256 512; do \
		sips -z $$s $$s .build/icon_1024.png --out .build/lyra.iconset/icon_$${s}x$${s}.png >/dev/null; \
	done
	@sips -z 32 32 .build/icon_1024.png --out .build/lyra.iconset/icon_16x16@2x.png >/dev/null
	@sips -z 64 64 .build/icon_1024.png --out .build/lyra.iconset/icon_32x32@2x.png >/dev/null
	@sips -z 256 256 .build/icon_1024.png --out .build/lyra.iconset/icon_128x128@2x.png >/dev/null
	@sips -z 512 512 .build/icon_1024.png --out .build/lyra.iconset/icon_256x256@2x.png >/dev/null
	@sips -z 1024 1024 .build/icon_1024.png --out .build/lyra.iconset/icon_512x512@2x.png >/dev/null
	iconutil -c icns .build/lyra.iconset -o app/Resources/AppIcon.icns

# AccentColor.colorset → Assets.car — the canonical app-accent mechanism.
# actool ships only with full Xcode (CLT ships a stub); DEVELOPER_DIR
# points tool resolution at it without a system-wide xcode-select.
XCODE_DEV := /Applications/Xcode.app/Contents/Developer
$(APP)/Contents/Resources/Assets.car: app/Resources/Assets.xcassets/AccentColor.colorset/Contents.json
	@mkdir -p $(APP)/Contents/Resources
	@if [ -x "$(XCODE_DEV)/usr/bin/actool" ]; then \
	    DEVELOPER_DIR=$(XCODE_DEV) actool \
	        --compile $(APP)/Contents/Resources \
	        --platform macosx --minimum-deployment-target 15.0 \
	        --output-format human-readable-text \
	        app/Resources/Assets.xcassets; \
	else \
	    echo "warning: Xcode not installed — skipping Assets.car (accent stays system blue)"; \
	fi

$(APPBIN): $(SWIFT_SRC) $(LIBDIR)/liblyra_ffi.a
	@mkdir -p $(APP)/Contents/MacOS $(APP)/Contents/Resources
	$(SWIFTC) -O -parse-as-library \
	    -import-objc-header modules/CLyraFFI/lyra.h \
	    -I modules/CLyraFFI \
	    $(SWIFT_SRC) \
	    -L $(LIBDIR) -llyra_ffi -lz \
	    -framework SwiftUI -framework AppKit -framework UniformTypeIdentifiers \
	    -framework AudioToolbox -framework CoreAudio -framework CoreFoundation \
	    -framework AVFoundation -framework CoreServices -framework MediaPlayer \
	    -framework SystemConfiguration -framework Security \
	    -o $(APPBIN)

$(APP)/Contents/Info.plist: Info.plist
	@cp Info.plist $(APP)/Contents/

sign: darwin-only $(APPBIN)
	codesign --force --sign - --entitlements entitlements.plist $(APP)

run: app
	open $(APP)

# Pseudo-HMR: incremental build (Swift-only ≈ seconds) then relaunch.
# True hot reload needs Xcode previews or InjectionIII — the latter wants
# disable-library-validation, which conflicts with our signing posture.
dev: app
	@pkill -f 'Lyra.app/Contents/MacOS/Lyra' 2>/dev/null || true
	@open $(APP)

# Auto-rebuild+relaunch on Swift source save (watchexec via mise).
watch:
	watchexec -w app/Sources -e swift -- make dev

clean:
	rm -rf .build $(LIBDIR)
	$(CARGO) clean
