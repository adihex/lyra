# Lyra — no-xcodeproj build. Rust core → staticlib → swiftc → .app bundle → codesign.
# For release we'd move to XcodeGen for Sparkle's nested-signing (see BLUEPRINT § packaging).

CARGO    := mise exec -- cargo
SWIFTC   := xcrun swiftc
APP      := .build/Lyra.app
APPBIN   := $(APP)/Contents/MacOS/Lyra
LIBDIR   := app/.libs
SWIFT_SRC:= $(wildcard app/Sources/LyraApp/*.swift app/Sources/LyraApp/*/*.swift)

.PHONY: all rust app sign run clean check

all: app

check:
	$(CARGO) check --workspace

rust:
	$(CARGO) build -p lyra-ffi
	@mkdir -p $(LIBDIR)
	@cp target/debug/liblyra_ffi.a $(LIBDIR)/

app: rust $(APPBIN) $(APP)/Contents/Info.plist $(APP)/Contents/Resources/AppIcon.icns $(APP)/Contents/Resources/Assets.car sign

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
	    -L $(LIBDIR) -llyra_ffi \
	    -framework SwiftUI -framework AppKit -framework UniformTypeIdentifiers \
	    -framework AudioToolbox -framework CoreAudio -framework CoreFoundation \
	    -framework AVFoundation -framework CoreServices -framework MediaPlayer \
	    -framework SystemConfiguration -framework Security \
	    -o $(APPBIN)

$(APP)/Contents/Info.plist: Info.plist
	@cp Info.plist $(APP)/Contents/

sign: $(APPBIN)
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
