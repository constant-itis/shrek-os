cmake_minimum_required(VERSION 3.19)

project(shrek-caelestia-blobs LANGUAGES CXX)

set(CMAKE_CXX_STANDARD 20)
set(CMAKE_CXX_STANDARD_REQUIRED ON)
set(CMAKE_CXX_EXTENSIONS OFF)

find_package(Qt6 REQUIRED COMPONENTS Core Gui Qml Quick ShaderTools)

qt_standard_project_setup()

set(BLOBS_DIR "${CMAKE_CURRENT_SOURCE_DIR}/plugin/src/Caelestia/Blobs")

qt_add_qml_module(caelestia-blobs
    URI Caelestia.Blobs
    VERSION 1.0
    SOURCES
        "${BLOBS_DIR}/blobgroup.cpp"
        "${BLOBS_DIR}/blobgroup.hpp"
        "${BLOBS_DIR}/blobshape.cpp"
        "${BLOBS_DIR}/blobshape.hpp"
        "${BLOBS_DIR}/blobrect.cpp"
        "${BLOBS_DIR}/blobrect.hpp"
        "${BLOBS_DIR}/blobinvertedrect.cpp"
        "${BLOBS_DIR}/blobinvertedrect.hpp"
        "${BLOBS_DIR}/blobmaterial.cpp"
        "${BLOBS_DIR}/blobmaterial.hpp"
)

target_include_directories(caelestia-blobs PRIVATE "${BLOBS_DIR}")
target_link_libraries(caelestia-blobs PRIVATE Qt6::Quick)

# Make the module self-contained and relocatable. qt_add_qml_module produces TWO libraries: the
# backing lib (caelestia-blobs) and the loadable plugin (caelestia-blobsplugin). By default the
# backing lib lands in the build dir and the plugin gets an absolute RUNPATH into that build dir —
# which does NOT exist inside the sealed image, so the plugin fails to load its backing lib and the
# blobs never appear. Put the backing lib in the SAME dir as the plugin (the QML module dir) and give
# both an $ORIGIN rpath, so wherever the Caelestia/Blobs/ dir is copied (preview out/ or sealed
# /usr/lib/qt6/qml) the plugin finds its backing lib right next to it.
if(NOT DEFINED QT_QML_OUTPUT_DIRECTORY)
    set(QT_QML_OUTPUT_DIRECTORY "${CMAKE_BINARY_DIR}")
endif()
set(_blobs_module_dir "${QT_QML_OUTPUT_DIRECTORY}/Caelestia/Blobs")
set_target_properties(caelestia-blobs PROPERTIES
    LIBRARY_OUTPUT_DIRECTORY "${_blobs_module_dir}"
    BUILD_WITH_INSTALL_RPATH ON
    INSTALL_RPATH "$ORIGIN"
)
set_target_properties(caelestia-blobsplugin PROPERTIES
    BUILD_WITH_INSTALL_RPATH ON
    INSTALL_RPATH "$ORIGIN"
)

# BASE makes the resource alias relative to the shaders' own dir, so the compiled .qsb land at
# ":/shaders/blob.frag.qsb" — exactly what BlobMaterialShader::setShaderFileName() asks for.
# Without BASE the absolute FILES paths aliased the resource under ":/work/third_party/..." and the
# material could not find its shaders (blank blobs on the software backend, segfault under GL).
qt_add_shaders(caelestia-blobs "blob_shaders"
    BATCHABLE OPTIMIZED NOHLSL NOMSL
    GLSL "300es,330"
    PREFIX "/"
    BASE "${BLOBS_DIR}"
    FILES
        "${BLOBS_DIR}/shaders/blob.frag"
        "${BLOBS_DIR}/shaders/blob.vert"
)
