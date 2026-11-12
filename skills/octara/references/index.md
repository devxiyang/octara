# Octara index

Every name an app can use, grouped. Signatures and docs: `octara vocab <name>`.

- `{N}` = the steps 0,0p5,1,1p5,2,2p5,3,3p5,4,5,6,7,8,9,10,11,12,14,16,20,24,28,32,36,40,44,48,52,56,60,64,72,80,96 (`0p5` = 0.5; Tailwind's scale)
- `{M}` = the steps 0,0p5,1,1p5,2,2p5,3,3p5,4,5,6,7,8,9,10,11,12,14,16,20,24 (`0p5` = 0.5; Tailwind's scale)

Prelude (`use octara::prelude::*`), by module:
- app: App, Drag, DragPhase, Event, Key, KeyEvent, Modifiers, Window, WindowId
- canvas: Canvas
- icon: Icon
- image: Image, ObjectFit
- kernel: linear_color_stop, linear_gradient, rgb, Align, BoxShadow, Breakpoint, Color, ContainerStep, Family, Fill, Length, Paint, Placement, RadiusStep, ShadowStep, Side, State, TextStep, Theme, Token
- motion: Curve, Spring
- native: NativeEvent, Slot
- paint: Rect
- path: point, FillRule, LineCap, LineJoin, Path, Stroke, Transform
- platform: WindowKind, WindowOptions
- reactive: list, live_text, memo, provide, state, untrack, use_context, watch, Derived, FlatItem, FlatKey, GroupInfo, Groups, List, LiveText, Memo, ReadSignal, Signal, Source
- task: background, channel, interval, resource, sleep, spawn, Load, Receiver, Resource, Sender, Task
- view: breakpoint, canvas, col, component, component_keyed, each, el, icon, instant, native_view, on_command, px, relative, rems, row, scroll_to, set_theme, settle_motion, show, show_else, size_of, sp, spacer, text, text_input, viewport, virtual_each, web_view, El, IntoElement, IntoProp, IntoText, Mount, Patch, Styled, View
- window: close_window, current_window, open_window, windows

Members of prelude types (`.name(..)`):
- App: advance, advance_to, close_window, diagnostic_counts, diagnostics, hot_reload, new, next_deadline, now, open_window, pending, platform, poll, register_fallback_font, register_font, reset_work, run, run_until_idle, send, set_check_consistency, set_painter, set_theme, set_reduced_motion, set_target_platform, set_text_engine, settle, settle_motion, target_platform, theme, use_system_fonts, window, windows, work
- Canvas: clip, fill, fill_with, height, image, into_ops, new, ops, rect, size, stroke, text, transform, width
- ContainerStep: Lg, Md, Sm, Xl, Xl2, Xl3, Xl4, Xl5, Xl6, Xl7, Xs, Xs2, Xs3
- Derived: filter, group_by, is_empty, join, keys, len, map, sort_by, source
- El: a11y_hidden, active, autofocus, bg, border_color, checked, child, children, description, disabled, expanded, focus, focus_{group,visible}, focusable, group, group_hover, hover, hover_alone, id, img, key_context, label, modal, multiline, object_fit, on_{command,drag,input,key,native,press,submit,toggle}, on_double_press, on_long_press, on_secondary_press, opacity, overlay, props, revealed, role, rows, screen, secure, selected, slot, stroke_width, style_when, submit_on_enter, tab_index, text_color, value, when_{expanded,large,medium,touch}, when_extra_large
- Event: Blur, Focus, Hover, Input, Press, Scroll, ScrollTo, Send, Toggle
- Icon: bounds, name, named, path, shapes, stroked, svg, view_box
- Key: Backspace, Char, Delete, Down, End, Enter, Escape, F, Home, Left, Menu, PageDown, PageUp, Right, Space, Tab, Up
- List: all, filter, group_by, index_of, insert, is_empty, item, join, keys, last_key, len, map, push, remove, retain, sort_by, source, update
- LiveText: append, get, get_untracked, is_empty, len, log_len, new, set, splice, with
- Modifiers: ALT, CTRL, META, SHIFT
- Path: add_polygon, append, arc_to, bounds, circle, close, commands, contains, content_hash, cubic_bezier_to, curve_to, ellipse, is_empty, line_to, move_to, new, rect, rounded_rect, svg, to_svg, transform
- Placement: ABOVE, BELOW, RIGHT, align, fit, gap, margin, place, side
- RadiusStep: Lg, Md, Sm, Xl, Xl2, Xl3, Xl4, Xs
- Rect: bottom, h, intersect, is_empty, new, right, w, x, y
- Signal: get, get_untracked, new, read_only, set, update, with, with_untracked
- Source: filter, group_by, is_empty, item, join, keys, len, log_len, map, sort_by
- Spring: GLIDE, MORPH, NAMED, POP, REFLOW, SETTLE, SNAP, bounce, duration, new
- State: CHECKED, DISABLED, EXPANDED, FOCUSED, FOCUS_VISIBLE, HOVER, PRESSED, SELECTED
- Stroke: cap, dash, dash_offset, join, miter_limit, new, reach, width
- Styled (trait; El, Patch): absolute, aspect, aspect_{square,video}, bg_paint, border, border_{0,2,4,8,paint,w}, bottom, bottom_{0,0p5,1,2,3,4,6,8,12,16,full}, bottom_1_2, col_{end,span,start}, col_end_auto, col_span_full, col_start_auto, color_motion, flex, flex_{1,auto,basis,col,grow,none,nowrap,row,shrink,wrap}, flex_col_reverse, flex_row_reverse, flex_shrink_0, font_{black,bold,extrabold,extralight,family,light,medium,mono,normal,sans,semibold,thin,weight}, gap, gap_{M,x,y}, gap_x_{M}, gap_y_{M}, grid, grid_{cols,rows}, h, h_{N,auto,full}, h_1_{2,3,4}, h_2_3, h_3_4, hidden, inset, inset_{0,0p5,1,2,3,4,6,8,12,16,x,y}, items_{baseline,center,end,start,stretch}, justify_{around,between,center,end,evenly,start}, layout_motion, leading_{loose,none,normal,relaxed,snug,tight}, left, left_{0,0p5,1,2,3,4,6,8,12,16,full}, left_1_2, line_clamp, line_height, m, m_{N,auto}, map, max_h, max_w, max_h_{1,2,4,8,12,16,20,24,32,40,48,64,72,80,96,full}, max_w_{2xl,2xs,3xl,3xs,4xl,5xl,6xl,7xl,full,lg,md,sm,xl,xs}, mb, mb_{N,auto}, min_h, min_w, min_h_{0,1,2,4,8,12,16,20,24,32,40,48,64,80,96}, min_w_{0,1,2,4,8,12,16,20,24,32,40,48,64,80,96,full}, ml, ml_{N,auto}, motion, motion_none, mr, mr_{N,auto}, mt, mt_{N,auto}, mx, mx_{N,auto}, my, my_{N,auto}, opacity_{0,25,50,75,100,motion,value}, overflow_{auto,clip,hidden,scroll,visible}, overflow_x_{auto,hidden,scroll}, overflow_y_{auto,hidden,scroll}, p, p_{N}, pb, pb_{N}, pl, pl_{N}, pr, pr_{N}, pt, pt_{N}, px, px_{N}, py, py_{N}, relative, right, right_{0,0p5,1,2,3,4,6,8,12,16,full}, right_1_2, rounded, rounded_{2xl,3xl,4xl,full,lg,md,none,sm,xl,xs}, row_{end,span,start}, row_end_auto, row_span_full, row_start_auto, self_{auto,center,end,start,stretch}, shadow, shadow_{2xl,2xs,lg,md,none,sm,xl,xs}, size, size_{N,full}, style, text_{2xl,3xl,4xl,5xl,6xl,7xl,8xl,9xl,base,ellipsis,lg,paint,size,sm,step,xl,xs}, top, top_{0,0p5,1,2,3,4,6,8,12,16,full}, top_1_2, truncate, w, w_{N,2xl,2xs,3xl,3xs,auto,full,lg,md,sm,xl,xs}, w_1_{2,3,4,5,6}, w_2_3, w_2_5, w_3_4, w_3_5, w_4_5, w_5_6, when, when_some, whitespace_normal, whitespace_nowrap
- TextStep: Base, Lg, Sm, Xl, Xl2, Xl3, Xl4, Xl5, Xl6, Xl7, Xl8, Xl9, Xs
- Theme: breakpoint, breakpoint_{of,width}, color, container, container_of, dark, family, font_mono, font_sans, get, light, line_height, name, radius, radius_of, rem, shadow, shadow_of, spacing, text, text_of, try_get
- Token: Accent, AccentHover, AccentText, BUILTIN, Background, Border, Custom, Danger, Focus, Surface, SurfaceHover, SurfacePressed, Text, TextMuted, custom
- Transform: IDENTITY, apply, invert, rotate, scale, then, translate, x, xx, xy, y, yx, yy
- Window: accessibility_{action,tree,update}, app, at_sizes, bind, breakpoint, capture_{awaiting,begin,display,finish}, caret, clear_forced, click_at, clipboard_text, commands, dispatch, display_list, double_tap, drag, force_state, headless, headless_on, id, inspect, invoke, is_closed, kernel, kernel_mut, key_event, keymap, layout_issues, long_press, native_{event,host,size,snapshot}, options, platform_window, pointer, pointer_{down,leave,move,up}, press_key, replace_text, resize, restart_accessibility, secondary_tap, set_{composition,keymap,selection,tuning}, size, swipe, tap, tap_with, text_field, tuning, type_text, unsettled, wheel
- Align: Center, End, Start
- BoxShadow: blur_radius, color, new, offset, spread_radius
- Breakpoint: ALL, Compact, Expanded, ExtraLarge, Large, Medium
- Curve: Fade, Instant, Spring
- Drag: dx, dy, phase, x, y
- DragPhase: Cancel, End, Move, Start
- Family: Mono, Named, Sans
- Fill: Linear, Solid
- FillRule: EvenOdd, NonZero
- FlatItem: Header, Item, header, item
- FlatKey: Header, Item
- GroupInfo: key, len
- Groups: flatten, is_empty, len, members, source
- Image: decode, frame, id, rgba, set, size
- IntoElement (trait; &str, El, String, View): into_view
- IntoProp (trait; &str, Color, Fill, Paint, String, Token, Value, bool, f32, generic impls): into_prop
- IntoText (trait; &LiveText, LiveText, generic impls): into_text
- KeyEvent: code, key, modifiers, new, repeat
- Length: Auto, Container, Pct, Px, Rems, Spacing, px
- LineCap: Butt, Round, Square
- LineJoin: Bevel, Miter, Round
- Load: error, loading, value
- Memo: get, get_untracked, with
- NativeEvent: data, name
- ObjectFit: Contain, Cover, Fill, None, ScaleDown, place
- Paint: Color, Token
- Patch: bg, border_color, opacity, text_color
- ReadSignal: get, get_untracked, with, with_untracked
- Receiver: named, on, recv
- Resource: error, get, loading, refetch, value
- Sender: send
- ShadowStep: Lg, Md, Sm, Xl, Xl2, Xs, Xs2
- Side: Bottom, Left, Right, Top
- Slot: Overlay, Underlay
- Task: cancel, detach, is_finished
- View: Dyn, El, Empty, Fragment
- WindowKind: Dialog, Normal
- WindowOptions: kind, new, size, title

Other crates and modules:
- octara_kit: Model, button, checkbox, ghost_button, primary_button, select, switch, text_field, toggle_button
- octara_shell: Options, ShellEvent, run, run_with
- octara::diagnostics: Area, CODES, Code, Diagnostic, Severity, code
- octara_fonts: cjk
- octara_render: Bitmap, CpuRenderer, GpuRenderer, cpu, glyphs, gpu, paths, reference_painter, screenshot, shape, wgpu
- commands (`App::invoke`, `on_command`): focus.{dismiss,down,first,last,left,next,prev,right,up}, press, submit, menu, edit.{copy,cut,delete_backward,delete_forward,delete_to_line_start,delete_to_start,delete_word_backward,delete_word_forward,move_{down,left,right,up},move_page_down,move_page_up,move_to_end,move_to_start,move_to_line_end,move_to_line_start,move_word_left,move_word_right,newline,paste,redo,select_{all,down,left,right,up},select_page_down,select_page_up,select_to_end,select_to_start,select_to_line_end,select_to_line_start,select_word_left,select_word_right,undo}
- diagnostic codes (`octara explain`): STRICT_READ_UNTRACKED, WRITE_IN_BUILD, WRITE_IN_DERIVE, READ_DISPOSED, WRITE_DISPOSED, CYCLE, FLUSH_RUNAWAY, NO_OWNER, DUPLICATE_STATE_KEY, STATE_TYPE_CHANGED, UNKEYED_STATE, VIEW_READS_STATE, JOIN_FANOUT, DUPLICATE_KEY, AMBIGUOUS_COMPONENT, DUPLICATE_COMPONENT_KEY, EACH_ROW_NOT_ELEMENT, VIRTUAL_LIST_NOT_SCROLLABLE, LAYOUT_LOOP, OVERFLOW, COLLAPSED, TEXT_TRUNCATED, NATIVE_COVERED, OVERLAY_CLAMPED, UNNAMED_CONTROL, UNLABELED_CANVAS, UNLABELED_NATIVE_VIEW, SMALL_TARGET, LOW_CONTRAST, NOT_SETTLED, BAD_REQUEST, NO_SUCH_ELEMENT, STEP_FAILED, EXPECTATION_FAILED, CAPTURE_FAILED, MISSING_TOKEN
