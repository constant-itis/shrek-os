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

qt_add_shaders(caelestia-blobs "blob_shaders"
    BATCHABLE OPTIMIZED NOHLSL NOMSL
    GLSL "300es,330"
    PREFIX "/"
    FILES
        "${BLOBS_DIR}/shaders/blob.frag"
        "${BLOBS_DIR}/shaders/blob.vert"
)
