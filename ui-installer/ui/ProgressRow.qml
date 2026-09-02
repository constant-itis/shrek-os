import QtQuick
import QtQuick.Layouts
import "../theme"

// One line in the install progress list. state_: done | now | pending. pct >= 0 renders a progress bar.
RowLayout {
    id: r
    property string state_: "pending"
    property string title: ""
    property string sub: ""
    property string index_: ""
    property int pct: -1
    spacing: 14

    Rectangle {
        width: 26
        height: 26
        radius: 13
        Layout.alignment: Qt.AlignTop
        color: r.state_ === "done" ? Tokens.accentDim : Tokens.surface
        border.width: 1
        border.color: r.state_ === "done" ? Tokens.accentDim : r.state_ === "now" ? Tokens.accent : Tokens.outline
        Text {
            anchors.centerIn: parent
            text: r.state_ === "done" ? "✓" : r.state_ === "now" ? "●" : r.index_
            font.family: Tokens.fontMono
            font.pixelSize: Tokens.fontSmall
            color: r.state_ === "done" ? Tokens.accentText : r.state_ === "now" ? Tokens.accent : Tokens.muted
        }
    }

    ColumnLayout {
        spacing: 2
        Layout.fillWidth: true
        Text {
            text: r.title
            color: r.state_ === "pending" ? Tokens.muted : Tokens.textPrimary
            font.family: Tokens.fontFamily
            font.pixelSize: Tokens.fontBody
        }
        Text {
            text: r.sub
            color: Tokens.muted
            font.family: Tokens.fontMono
            font.pixelSize: Tokens.fontSmall
        }
        Rectangle {
            visible: r.pct >= 0
            Layout.topMargin: 6
            Layout.preferredWidth: 400
            height: 6
            radius: 3
            color: Tokens.surfaceRaised
            Rectangle {
                width: parent.width * Math.max(0, Math.min(100, r.pct)) / 100
                height: parent.height
                radius: 3
                color: Tokens.accent
            }
        }
    }

    Text {
        visible: r.pct >= 0
        text: r.pct + "%"
        color: Tokens.textSecondary
        font.family: Tokens.fontMono
        font.pixelSize: Tokens.fontSmall
        Layout.alignment: Qt.AlignTop
    }
}
