import QtQuick

// The Shrek helmet mark. PNG (not SVG) so it renders without the qtsvg image plugin, which the headless
// render container does not install.
Image {
    property int size: 24
    width: size
    height: size
    sourceSize.width: size * 2
    sourceSize.height: size * 2
    fillMode: Image.PreserveAspectFit
    smooth: true
    source: Qt.resolvedUrl("../assets/helmet.png")
}
