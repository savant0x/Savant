# Hover Animated Icon Pack — Agent Reference

> **Audience:** coding agents working in the Savant dashboard (`src/`, Next.js 15 App Router + React 19 + Tauri).
> **Source:** [Its Hover](https://www.itshover.com/icons) — `motion/react`-based, hover-animated SVG icons.
> **Install FID:** `dev/fids/FID-2026-07-14-027-hover-icons-pack-install.md`

This document is the single source of truth for using the icon pack. It is written for agents: exact import paths, copy-paste code, the public API, and every gotcha that will otherwise cost you a build failure.

---

## 1. TL;DR (cheat sheet)

```tsx
// Option A — one known icon (preferred; tree-shakeable)
import HeartIcon from "@/components/icons/heart-icon";

// Option B — iterate / look up dynamically
import { iconRegistry, iconNames } from "@/components/icons";
const Icon = iconRegistry[name]; // name is a string from iconNames
```

- Icons live in `src/components/icons/*.tsx` (273 components + `types.ts` + `index.ts`).
- **Mandatory dependency:** `motion` (installed, `^12.42.2`). Do **not** remove it.
- **Mandatory directive:** every icon is a **client component** (`"use client"`). It must render under a client boundary (a `"use client"` page/component). This is already true for `src/app/icons/page.tsx`.
- **Showcase route:** `/icons` renders the full grid with a name filter — open it to browse.
- **Do not hand-edit** `src/components/icons/*.tsx` or `index.ts`. They are generated; regenerate instead (see §8).

---

## 2. What was installed

| Artifact | Path | Notes |
| --- | --- | --- |
| 273 icon components | `src/components/icons/*.tsx` | One per icon. Verbatim upstream code + `"use client"`. |
| Shared types | `src/components/icons/types.ts` | `AnimatedIconProps`, `AnimatedIconHandle`, `DEFAULT_STROKE_WIDTH`, `scaledStrokeWidth`, `IconEasing`. Imported by every icon via `./types`. |
| Barrel / registry | `src/components/icons/index.ts` | Re-exports all icons; exposes `iconRegistry` and `iconNames`. **Auto-generated — do not edit.** |
| Showcase route | `src/app/icons/page.tsx` | Client page; searchable grid of all icons. |
| Dependency | `package.json` → `motion@^12.42.2` | Runtime dep of every icon (`import { motion, useAnimate } from "motion/react"`). |

The icons were fetched from the official `itshover/itshover` GitHub registry (`public/r/*.json`), not via 273 manual `npx shadcn add` calls.

---

## 3. Public API

### 3.1 Named import (recommended)

```tsx
import HeartIcon from "@/components/icons/heart-icon";
import PlugConnectedIcon from "@/components/icons/plug-connected-icon";

export function LikeButton() {
  return <HeartIcon size={24} className="text-rose-500" />;
}
```

The import path is `./<file-basename-without-ext>` relative to `src/components/icons/`. The file basename is the icon's kebab-case name from the registry (e.g., `heart-icon`, `plug-connected-icon`).

### 3.2 Registry (dynamic lookup / iteration)

```tsx
import { iconRegistry, iconNames } from "@/components/icons";

// iconNames: string[] — every registered key (sorted at definition time)
// iconRegistry: Record<string, ComponentType<AnimatedIconProps>>

{iconNames.map((name) => {
  const Icon = iconRegistry[name];
  return <Icon key={name} size={20} />;
})}
```

Use this only when you need dynamic rendering (e.g., a picker, a docs page, a settings icon map). For a single static icon, prefer the named import (§3.1) so bundlers can tree-shake.

### 3.3 Imperative animation control (optional)

Each icon forwards a ref exposing `startAnimation()` / `stopAnimation()`. The default behavior already animates on hover, so you only need this for programmatic control (e.g., trigger on click, on state change).

```tsx
import { useRef } from "react";
import HeartIcon from "@/components/icons/heart-icon";
import type { AnimatedIconHandle } from "@/components/icons/types";

function Pulse() {
  const ref = useRef<AnimatedIconHandle>(null);
  return (
    <button onClick={() => ref.current?.startAnimation()}>
      <HeartIcon ref={ref} />
    </button>
  );
}
```

### 3.4 Props (`AnimatedIconProps`)

Defined in `src/components/icons/types.ts`. All optional.

| Prop | Type | Default | Notes |
| --- | --- | --- | --- |
| `size` | `number \| string` | `24` | Pixel size or any CSS size string. |
| `color` | `string` | `"currentColor"` | SVG `stroke` color. Inherit with `currentColor`. |
| `strokeWidth` | `number` | `2` | SVG stroke width. |
| `className` | `string` | `""` | Extra classes (color, sizing, layout). |

`AnimatedIconProps` also extends the standard `SVGProps<SVGSVGElement>` (minus `ref` and a few animation/ drag handlers), so any normal SVG attribute (`onClick`, `aria-*`, `fill`, etc.) is accepted.

> **Gotcha — `CustomAnimation` icons:** a small number of icons declare *additional required* animation props (e.g. rotation/`CustomAnimation`). They are still assignable through the registry and accept the standard props; you only need the extra props if you want to tune those specific animations. Treat them like any other icon for normal usage.

---

## 4. Usage patterns (copy-paste)

### In a server component? No.

Icons are client components. Rendering them inside a Server Component throws a React error. Always render them from a `"use client"` module. The dashboard pages (`src/app/*.tsx`) that need icons must either be `"use client"` or receive the icon from a client child.

### With HeroUI

The pack is independent of `@heroui/react`. You can drop an icon inside a HeroUI `<Card>` (as the showcase does) or use it standalone.

```tsx
"use client";
import { Card } from "@heroui/react";
import GearIcon from "@/components/icons/gear-icon";

export function SettingsTile() {
  return (
    <Card className="flex items-center gap-2 p-4">
      <GearIcon size={20} className="text-foreground" />
      <span>Settings</span>
    </Card>
  );
}
```

### Mapping a name to an icon (e.g., config-driven UI)

```tsx
import { iconRegistry } from "@/components/icons";

function renderIcon(name: string, size = 20) {
  const Cmp = iconRegistry[name];
  return Cmp ? <Cmp size={size} /> : null;
}
```

---

## 5. The `/icons` showcase

`src/app/icons/page.tsx` is a client route rendering every icon in a responsive grid with a live name filter. Use it to:
- Visually confirm an icon exists before referencing it.
- Copy the component name / import path.
- See the hover animation behavior.

Run `npm run dev` and open `http://localhost:3000/icons`.

---

## 6. Naming rules (read this before guessing an import)

- **File name → import path:** kebab-case file basename. `heart-icon.tsx` → `import HeartIcon from "@/components/icons/heart-icon"`.
- **Registry key → PascalCase of the file name.** `heart-icon` → `HeartIcon`. This is derived from the **filename**, not the component's internal `displayName` (they almost always match, but the key is file-based and guaranteed unique).
- **Not all keys end in `Icon`.** Because keys come from filenames, several do **not** carry an `Icon` suffix. Examples you will trip over if you assume a suffix:
  `BulbSvg`, `CreditCard`, `DownChevron`, `PhoneVolume`, `QrcodeSvg`, `QuestionMark`, `RightChevron`, `ShieldCheck`, `TravelBag`, `TreeIcon`, `CursorIdeIcon`, `HostelIcon`, `GrokIcon`, `GeminiIcon`, `VercelIcon`, `NotionIcon`, `RailwayIcon`.
  → **Do not assume `<X>Icon` exists. Look it up in `iconNames` or `/icons`.**
- To find the exact key for an icon, grep the barrel or the showcase:
  ```bash
  rg "from \"./heart-icon\"" src/components/icons/index.ts
  # or at runtime: Object.keys(iconRegistry)
  ```

---

## 7. Gotchas (the things that break builds)

1. **`motion` must stay installed.** Removing it breaks every icon (`Cannot find module 'motion/react'`).
2. **`"use client"` is required.** The icon files already have it. If you ever copy an icon elsewhere, add `"use client";` at the top.
3. **Don't render inside Server Components.** Route the icon through a client component.
4. **Don't edit generated files.** `src/components/icons/*.tsx` and `index.ts` are produced by the install script. Hand edits are lost on regen and will drift from upstream.
5. **Registry typing uses `as unknown as`.** The barrel casts each icon to `ComponentType<AnimatedIconProps>` (some icons add required animation props). Do not "fix" this to a plain `any` (lint) or remove the cast (type error).
6. **Tree-shaking:** prefer named imports (`@/components/icons/heart-icon`) over `iconRegistry` for hot paths; importing the barrel pulls in all 273 components.
7. **`next lint` is currently non-functional in this repo** (no ESLint config; it prompts interactively). Verify changes with `npm run build` + `npm run type-check`, not `npm run lint`.

---

## 8. Adding / updating icons (regeneration)

Icons are fetched from the upstream registry. To refresh or add more:

1. Fetch the registry index: `https://api.github.com/repos/itshover/itshover/contents/public/r`
2. For each `*.json`, download `download_url`, parse `files`, and for the `.tsx` component write it to `src/components/icons/<name>.tsx` with `"use client";` prepended (if absent).
3. Write `src/components/icons/types.ts` from the registry's shared `icons/types.ts`.
4. Regenerate `src/components/icons/index.ts`:
   - Key each icon by **unique PascalCase-of-filename** (avoids upstream name collisions).
   - `import X from "./<basename>"` and `export const iconRegistry: Record<string, IconComponent> = { X: X as unknown as IconComponent, ... }`.
   - `type IconComponent = ComponentType<AnimatedIconProps>` (imported from `./types`).
5. Verify: `npm run type-check && npm run build`.

The original install used two PowerShell scripts (network fetch + index regen). Favor this scripted approach over manual edits so the pack stays reproducible and verbatim.

---

## 9. Full icon list (273)

Canonical, runtime source of truth is `iconNames` from `@/components/icons`. Listed here for grep/discovery.

<details><summary>Expand all 273 icon keys</summary>

```
AccessibilityIcon, AirplaneIcon, AlarmClockPlusIcon, AlignCenterIcon, AlignVerticalSpaceAroundIcon, AmbulanceIcon, AmpersandIcon, AngryIcon, AnnoyedIcon, AppleBrandLogo, ArrowBackIcon, ArrowBackUpIcon, ArrowBigDownDashIcon, ArrowBigDownIcon, ArrowBigLeftDashIcon, ArrowBigLeftIcon, ArrowBigRightDashIcon, ArrowBigRightIcon, ArrowBigUpDashIcon, ArrowBigUpIcon, ArrowDown01Icon, ArrowDown10Icon, ArrowDownAZIcon, ArrowNarrowDownDashedIcon, ArrowNarrowDownIcon, ArrowNarrowLeftDashedIcon, ArrowNarrowLeftIcon, ArrowNarrowRightDashedIcon, ArrowNarrowRightIcon, ArrowNarrowUpDashedIcon, ArrowNarrowUpIcon, AtSignIcon, BananaIcon, BatteryChargingIcon, BatteryIcon, BatteryPauseIcon, BellOffIcon, BluetoothConnectedIcon, BookIcon, BookmarkIcon, BrainCircuitIcon, BrandAistudioIcon, BrandAnthropicIcon, BrandAwsIcon, BrandBagsFmIcon, BrandChromeIcon, BrandCursorIcon, BrandGeminiIcon, BrandGoogleIcon, BrandGrokIcon, BrandLmstudioIcon, BrandMidjourneyIcon, BrandNextjsIcon, BrandNotionIcon, BrandOllamaIcon, BrandOpenaiIcon, BrandPaypalIcon, BrandQwenIcon, BrandRailwayIcon, BrandReactIcon, BrandReactNativeIcon, BrandStripeIcon, BrandTelegramIcon, BrandThreadsIcon, BrandTwitchIcon, BrandVercelIcon, BrandWindowsIcon, BrandWordpressIcon, BrandXaiIcon, BrandZoomIcon, BrightnessDownIcon, BugIcon, BulbSvg, CameraIcon, CameraOffIcon, CandyCaneIcon, CartIcon, ChartBarIcon, ChartCovariateIcon, ChartHistogramIcon, ChartLineIcon, ChartPieIcon, CheckedIcon, ClockIcon, Cloud1Icon, Cloud2Icon, Cloud3Icon, CodeIcon, CodeXmlIcon, CoffeeIcon, CoinBitcoinIcon, CopyIcon, CopyOffIcon, CopyrightIcon, CpuIcon, CreditCard, CurrencyBitcoinIcon, CurrencyDollarIcon, CurrencyEthereumIcon, CurrencyEuroIcon, CurrencyRupeeIcon, CursorIdeIcon, DeviceAirpodsIcon, DialpadIcon, DinoIcon, DiscordIcon, DockerIcon, DotsHorizontalIcon, DotsVerticalIcon, DoubleCheckIcon, DownChevron, DownloadIcon, DrumIcon, ExpandIcon, ExternalLinkIcon, EyeIcon, EyeOffIcon, FacebookIcon, FigmaIcon, FileDescriptionIcon, FilledBellIcon, FilledCheckedIcon, FilterIcon, FlameIcon, FocusIcon, GamepadIcon, GaugeIcon, GearIcon, GeminiIcon, GhostIcon, GithubCopilotIcon, GithubIcon, GitlabIcon, GlobeIcon, GmailIcon, GolangIcon, GrokIcon, HandHeartIcon, HashtagIcon, HeartIcon, HistoryCircleIcon, HomeIcon, HostelIcon, HotelIcon, InfoCircleIcon, InstagramIcon, JavascriptIcon, KeyframesIcon, LayersIcon, LayoutBottombarCollapseIcon, LayoutDashboardIcon, LayoutSidebarRightCollapseIcon, LayoutSidebarRightIcon, LetterAIcon, LetterBIcon, LetterCIcon, LetterDIcon, LetterEIcon, LetterFIcon, LetterGIcon, LetterHIcon, LetterIIcon, LetterJIcon, LetterKIcon, LetterLIcon, LetterMIcon, LetterNIcon, LetterOIcon, LetterPIcon, LetterQIcon, LetterRIcon, LetterSIcon, LetterTIcon, LetterUIcon, LetterVIcon, LetterWIcon, LetterXIcon, LetterYIcon, LetterZIcon, LibraryIcon, LikeIcon, LinkedinIcon, LinkIcon, LocateIcon, LockIcon, LogoutIcon, MagnifierIcon, MailFilledIcon, MapPinIcon, MehIcon, MessageCircleIcon, MoonIcon, MousePointer2Icon, MysqlIcon, NodejsIcon, NotionIcon, PaintIcon, PartyPopperIcon, PassportIcon, PawPrintIcon, PenIcon, PhoneVolume, PinterestIcon, PlayerIcon, PlugConnectedIcon, PlugConnectedXIcon, PythonIcon, QrcodeIcon, QrcodeSvg, QuestionMark, QwenIcon, RadioIcon, RailwayIcon, RainbowIcon, RefreshIcon, RightChevron, RocketIcon, RosetteDiscountCheckIcon, RosetteDiscountIcon, RouterIcon, SatelliteDishIcon, SaveIcon, ScanBarcodeIcon, ScanHeartIcon, SendHorizontalIcon, SendIcon, ShieldCheck, ShoppingCartIcon, SimpleCheckedIcon, SkullEmoji, SlackIcon, SlidersHorizontalIcon, SnapchatIcon, SoupIcon, SparklesIcon, SpotifyIcon, Stack3Icon, StackIcon, StarIcon, SubscriptIcon, TargetIcon, TelephoneIcon, TerminalIcon, ToggleIcon, TrashIcon, TravelBag, TreeIcon, TriangleAlertIcon, TrophyIcon, TruckElectricIcon, TwitterIcon, TwitterXIcon, TypescriptIcon, UnlinkIcon, UnorderedListIcon, UploadIcon, UserCheckIcon, UserIcon, UserPlusIcon, UsersGroupIcon, UsersIcon, VercelIcon, VinylIcon, Volume2Icon, VolumeXIcon, WalletIcon, WashingMachineIcon, WhatsappIcon, WifiIcon, WifiOffIcon, WorldIcon, XIcon, YoutubeIcon
```

</details>

---

## 10. Decision guidance for agents

- **Need one icon in a component?** Named import (§3.1). Best for bundle size.
- **Need a list/picker/docs?** `iconRegistry` + `iconNames` (§3.2).
- **Need to trigger animation on a non-hover event?** `ref` + `startAnimation()` (§3.3).
- **Not sure an icon exists?** Check `/icons` or `iconNames` — do **not** guess the name or assume an `Icon` suffix.
- **Changing an icon's look?** Pass `size` / `color` / `strokeWidth` / `className` props; do not fork the component file.
- **Refreshing the pack?** Regenerate per §8; never hand-edit generated files.
