# AppKit and Metal display-clock probe

The November 28, 2026 run used an Apple M4 on macOS 27.2 (26B123). The primary built-in Retina display ran at 60 Hz, and the extended P27QBC-RG ran at 144 Hz. AppKit and the active Core Graphics display modes reported those rates. The raw callback and presented timestamps, screen-change notification logs, and system configuration are in [the run archive](m4-macos27.2-2026-11-28.tar.gz).

## External display results

Rates are in Hz. The gap is the median positive-presentation interval in milliseconds.

| Clock and range | External callbacks | External presents | External gap | After move callbacks | After move presents | After move gap |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| NSView, interactive | 60.00 | 60.03 | 13.889 | 60.00 | 59.71 | 13.889 |
| NSView, default | 60.00 | 60.03 | 13.889 | 60.00 | 60.03 | 13.889 |
| NSScreen, interactive | 60.00 | 59.89 | 13.889 | 60.00 | 60.03 | 13.889 |
| NSScreen, default | 60.00 | 59.89 | 13.889 | 60.00 | 59.89 | 13.889 |
| NSScreen, low | 28.80 | 28.74 | 34.723 | 28.80 | 28.80 | 34.723 |
| CAMetalDisplayLink, interactive | 144.00 | 143.68 | 6.945 | 144.00 | 144.00 | 6.945 |
| CAMetalDisplayLink, fixed | 144.00 | 144.00 | 6.945 | 144.00 | 144.00 | 6.945 |
| CAMetalDisplayLink, default | 144.00 | 144.00 | 6.945 | 143.04 | 142.41 | 6.945 |
| CVDisplayLink, display ID | 144.05 | 143.68 | 6.945 | 144.06 | 144.00 | 6.945 |

On the built-in panel, the interactive and default cases ran at roughly 60 Hz with a 16.666 ms median presentation gap on both visits. NSScreen's low range ran at 30 Hz. On the external panel, the AppKit links reported an approximately 6.945 ms target-minus-timestamp interval even as callbacks arrived at 60 Hz. Actual presentations there commonly took two or three external refreshes, which explains the shorter median gap relative to the mean implied by 60 Hz.

Core Video and Metal both reached the panel's native cadence. The current host uses Core Video for interactive frames because it supplies the measured native cadence to the existing drawable acquisition path. The fixed-range Metal link had the cleanest standalone presentation run. Its callback supplies a drawable and a deadline to call `present`, which would require the renderer to take ownership of that drawable. The NSView link continues to handle the low-rate and idle states. Every Core Video drawable submitted in the measurement windows reported a positive presentation timestamp. One late Core Video callback after the move was logged without acquiring a drawable. The figures are observations from this configuration and workload.

## Octara host verification

An Octara desktop fixture ran a 14-second color transition while the same AppKit window moved from Retina to the external screen and back. Octara logged the delivered frame ticks with `OCTARA_TRACE_FRAME_CLOCK=1`. A driver moved the window and added an `MTLDrawable.addPresentedHandler` to each drawable returned by the host layer. The archive's `host/` directory contains the fixture, driver, raw log and summary script.

| Window position | Delivered tick Hz | Typical tick interval (ms) | Positive present Hz | Median present gap (ms) |
| --- | ---: | ---: | ---: | ---: |
| Built-in Retina | 60.00 | 16.666 | 60.00 | 16.666 |
| P27QBC-RG | 144.00 | 6.945 | 143.28 | 6.945 |
| Back on Retina | 60.00 | 16.666 | 60.00 | 16.666 |

The table uses 2.7-second settled windows for the tick target spans and 2.8-second windows for the positive presentation timestamps. Three drawables reported zero over the whole run, one before initial visibility and one at each screen move. After the transition settled on Retina, the clock delivered three ticks at the low rate and went quiet.

## Reproduction

From the repository root on a Mac with an active GUI login and an extended display:

```sh
results=$(mktemp -d /tmp/octara-clock.XXXXXX)
xcrun clang -fobjc-arc -O2 -Wall -Wextra -Werror \
    -Wno-deprecated-declarations -Wno-unused-parameter \
    -framework AppKit -framework QuartzCore -framework CoreVideo -framework Metal \
    tools/macos-display-clock/probe.m -o "$results/probe"
for driver in view screen metal-wide metal-fixed metal-default cv view-default screen-default screen-low; do
    for route in external move; do
        "$results/probe" "$driver" "$route" > "$results/$driver-$route.csv" 2> "$results/$driver-$route.log" || exit
    done
done
python3 tools/macos-display-clock/summarize.py "$results"
```

Each process opens one visible 480 × 320 point AppKit window on the fastest external screen, or moves the same window from the built-in screen to the external screen and back. Each visit lasts approximately four seconds. The window uses a layer-hosted `CAMetalLayer`, with drawable size adjusted for the display's backing scale and the default drawable count and asynchronous presentation settings. A single Metal render pass clears the drawable to a changing color on every eligible callback.

- The NSView link is created before the window is shown. NSScreen and Core Video links are recreated on `NSWindowDidChangeScreenNotification`. The Metal link stays on the same layer across the move.
- The interactive range sets minimum to `min(80, display_max)` and maximum and preferred to `display_max`. The fixed range sets all three to `display_max`. The default cases use `CAFrameRateRangeDefault`. The low range requests 24–40 Hz, preferring 30 Hz. All Metal cases use `preferredFrameLatency = 1`.
- The NSView, NSScreen, and Core Video cases call `nextDrawable` on the main thread and schedule `presentDrawable` on the rendering command buffer. The Metal case commits the render pass and calls `present` on the drawable supplied by the update. Its CSV `interval_or_lead` column contains the difference between the presentation target and the submission deadline.
- The callback timestamp is sampled at callback entry, on the Core Video thread for that case. The CSV also records the time the main thread starts processing it and the time drawable acquisition returns. A Core Video sample that reaches the main thread more than a refresh interval late is logged as stale and skips rendering.
- `MTLDrawable.addPresentedHandler` provides each drawable's actual `presentedTime`. The process allows one second for the last handlers. The report uses callbacks from 0.75 through 3.90 seconds into each stage, and matches their presentation rows by sequence number. It calculates rates over the first-to-last timestamp span and reports zero timestamps and missing handlers separately. The probe measures the operating system's presentation timestamps in a small window with a trivial Metal workload.

To reproduce the full table from the saved raw recordings, pass the archive to `summarize.py` in place of the results directory.
