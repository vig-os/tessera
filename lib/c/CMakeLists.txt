cmake_minimum_required(VERSION 3.16)
project(fd5 C CXX)

set(CMAKE_C_STANDARD 11)
set(CMAKE_CXX_STANDARD 17)

# Allow manual override when find_package cannot locate HDF5 (e.g. split nix outputs).
# Usage: cmake .. -DHDF5_INCLUDE_DIR=... -DHDF5_LIBRARY=...
if(HDF5_INCLUDE_DIR AND HDF5_LIBRARY)
    message(STATUS "Using manually specified HDF5:")
    message(STATUS "  include: ${HDF5_INCLUDE_DIR}")
    message(STATUS "  library: ${HDF5_LIBRARY}")
    set(HDF5_INCLUDE_DIRS "${HDF5_INCLUDE_DIR}")
    set(HDF5_C_LIBRARIES "${HDF5_LIBRARY}")
else()
    find_package(HDF5 REQUIRED COMPONENTS C)
endif()

add_library(fd5 STATIC
    src/sha256.c
    src/fd5_builder.c
    src/fd5_hash.c
    src/fd5_attr.c
    src/fd5_verify.c
)

target_include_directories(fd5
    PUBLIC include ${HDF5_INCLUDE_DIRS}
    PRIVATE src
)
target_link_libraries(fd5 PUBLIC ${HDF5_C_LIBRARIES})

# Tests
enable_testing()
add_executable(test_builder tests/test_builder.c)
target_link_libraries(test_builder fd5)
add_test(NAME test_builder COMMAND test_builder)
