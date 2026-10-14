# Steam controller layouts

[Back to the documentation index](README.md)

On SteamOS and Steam Deck, Steam Input can turn controller input into keyboard
and mouse input. This page describes a keyboard and mouse layout for FINAL
FANTASY XIV 1.23b that you build in Steam. The launcher ships no layout file
and changes no Steam layouts.

## Verification state

- A tester reported that keyboard and mouse bindings from a Steam Input layout
  worked in the launcher and in the game in Gaming Mode, using a Steam
  Controller on SteamOS 3.8.28. Per-key in-game results were not recorded.
  Other controllers are untested.
- The same tester reported that the game's own Gamepad option was unavailable
  and that an action set which outputs gamepad buttons did not control the
  game. This page does not claim a fix for that. See
  [Controller not detected in Gaming Mode](troubleshooting.md#controller-not-detected-in-gaming-mode).
- The key assignments below follow the Type A keyboard defaults described in
  archived documentation for the 1.x client. Whether 1.23b uses Type A by
  default is not confirmed. The function column cites the 1.x client's Basic
  Controls guide; no public link to it is given here, so the column cannot be
  checked from this page. Check the assignments in the game's key
  configuration and change the layout to match.
- The D-pad arrow-key row was not confirmed in game. A tester's exported copy
  of the layout had the D-pad as gamepad output instead of arrow keys.

## Set up

1. In Desktop Mode, add the launcher to Steam. See
   [Steam Deck](../packaging/flatpak/README.md#steam-deck) in the package
   README.
2. In Gaming Mode, open the shortcut's page, open its controller layout, and
   edit it. Steam's menu names differ between versions. A keyboard and mouse
   template, if your Steam offers one, is a reasonable starting point; the
   template name is community-reported.
3. Enter the bindings in the [table](#layout).
4. Put the keyboard and mouse bindings in the action set that is active when
   the game starts. See [Action sets](#action-sets).
5. Start the shortcut, sign in, and choose **Play**.

Steam stores a layout per shortcut, and community reports say it is filed
under the shortcut's name, so renaming the shortcut can detach its layout. A
second shortcut needs its own layout. Community reports also say that Desktop
Mode may apply the desktop layout instead of the shortcut's layout, so the
layout is intended for Gaming Mode.

## Layout

Bindings use Steam's names for inputs. Leave inputs your controller lacks
unbound. The last column lists the function of each key per the archived 1.x
Basic Controls guide (Type A), unverified for 1.23b. A dash means the function
was not checked.

| Input | Steam binding | Function (Type A, unverified for 1.23b) |
| --- | --- | --- |
| Left stick | W, A, S, D | Move the character |
| Left stick click (L3) | F | Toggle active and passive mode |
| Right stick | I (up), K (down), J (left), L (right) | Move the camera |
| Right stick click (R3) | V | Change the camera mode |
| D-pad | Arrow keys | Move the menu anchor; target nearest left or right; filter targets up or down |
| A | Enter | - |
| B | Escape | Cancel |
| X | Space | Open the chat prompt |
| Y | G, only if assigned in game | G is not a Type A default |
| L1 | Tap X, hold R | Toggle walking; toggle auto-run |
| R1 | Tap Tab, hold C | Change target; toggle target lock |
| L2 | Left Control | Modifier |
| R2 | Left Alt | Modifier |
| L4 | Right mouse click | Hold and drag to move the camera |
| R4 | Left mouse click | Select |
| L5 | Show Keyboard | Steam's on-screen keyboard (the Steam button plus X is a fallback, unverified here) |
| R5 | Take Screenshot (optional) | - |
| View (Select) | M | Open the map |
| Menu (Start) | Home | Open the main menu |
| Left trackpad | Directional swipe: up O, down P | Zoom the camera; the mouse wheel is an alternative |
| Right trackpad | As Mouse | Move the pointer |

Gaps in this layout:

- It has no action bar keys (0 to 9, Ctrl+0 to 9, Alt+0 to 9), no Shift+Up
  or Shift+Down bar cycling, no strafe keys, and no F1 to F12 keys. Use
  abilities with the mouse or the on-screen keyboard. The launcher reserves
  F11 and F12; see [Client controls](configuration.md#client-controls).
- L2 and R2 are modifiers with no number-key partner here. Using the
  on-screen keyboard while holding a trigger is unverified.
- A trigger binding with its analog output left enabled in a template also
  emits gamepad input alongside the key. This is a caution about the layout
  contents, not a measured result.

## Action sets

A layout can hold several action sets, and Steam starts in one of them. In
Steam's gamepad-style templates, the Default set outputs gamepad buttons and is
active at launch. A gamepad-output set sends no key presses, so keyboard and
mouse bindings in another set do nothing while it is active. A tester
reported being in that set when the game did not respond. This does not
establish that an action set is the reason the game's Gamepad option is
unavailable.

Either of these arrangements keeps keyboard and mouse bindings active when the
game starts:

- Put the keyboard and mouse bindings in the set that is active when the game
  starts.
- Keep a gamepad set for the launcher and a keyboard and mouse set for the
  game, and switch to the keyboard and mouse set before you choose **Play**.

The launcher's own gamepad navigation reads gamepad input, so it needs a set
that outputs gamepad buttons. A reported layout built from a gamepad template
binds the right trackpad as a mouse in both sets, so pointer input is
available in either.

### The Change Action Set trap

Steam's Change Action Set binding is easy to trigger in game. In a
reported layout built from a gamepad template, tapping Menu sends Home, but a
long press runs Change Action Set. In
the gamepad set it moves to the next set; in the keyboard and mouse set it
returns to the previous set. Holding Menu during play can therefore switch
back to the gamepad set, and the controls stop acting as keys until you switch
again. If you bind a set switch, avoid placing it on a button you hold in
game, and check the active set in Steam's controller overlay when input stops.

## Sharing layouts

Steam's own way to reuse a layout is to export it as a Personal layout, or to
publish it as a Community layout. For a non-Steam shortcut, community reports
describe temporarily renaming the shortcut to a title that has a Community
layout, saving that layout as a Personal layout, and renaming the shortcut
back. This is community-reported and unverified here.

Steam ties each layout to a controller type and hides layouts made for other
types. A layout built on one device may not appear on another, so rebuild it
on each device from the table. Exported layout files are not a supported
install format for this launcher.

## Troubleshooting

- The character does not move: check the active action set first.
- Keys type text instead of acting: the chat prompt is open. Press Escape.
- The layout is missing: it belongs to one shortcut. Select the layout for the
  shortcut that starts the launcher, and check whether the shortcut was
  renamed.
- The game's Gamepad option is unavailable, or the controller is not detected:
  see [Controller not detected in Gaming Mode](troubleshooting.md#controller-not-detected-in-gaming-mode).
