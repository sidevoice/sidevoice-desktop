import ActivityKit
import SwiftUI
import WidgetKit

/// The call's Live Activity, as the widget extension declares it (HANDOVER §6): the Lock Screen card, and the Dynamic
/// Island's expanded B (everything in one `.bottom` region), compact and
/// minimal. A stale activity (the app stopped updating it) draws the offline state (D-LA-OFFLINE).
@available(iOS 16.1, *)
public struct CallActivityWidget: Widget {
    public init() {}

    public var body: some WidgetConfiguration {
        ActivityConfiguration(for: CallActivityAttributes.self) { context in
            LockCard(state: Self.shown(context))
                .activitySystemActionForegroundColor(Color(hex: 0x545766))
        } dynamicIsland: { context in
            let state = Self.shown(context)
            return DynamicIsland {
                DynamicIslandExpandedRegion(.bottom) {
                    ExpandedBody(state: state)
                }
            } compactLeading: {
                CompactLeading(state: state)
            } compactTrailing: {
                CompactTrailing(state: state)
            } minimal: {
                MinimalFace(state: state)
            }
            .keylineTint(Tone.of(state.tonePhase, dark: true).fill)
            .widgetURL(CallLinks.call)
        }
    }

    /// What `context` draws: its state, or the offline state once stale.
    static func shown(_ context: ActivityViewContext<CallActivityAttributes>) -> CallState {
        if #available(iOS 16.2, *), context.isStale {
            return context.state.stale
        }
        return context.state
    }
}
