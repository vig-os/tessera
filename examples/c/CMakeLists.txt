cmake_minimum_required(VERSION 3.16)
project(fd5_example C)

set(CMAKE_C_STANDARD 11)

# fd5 library (built from source)
add_subdirectory(${CMAKE_CURRENT_SOURCE_DIR}/../../lib/c fd5_lib)

add_executable(create_fd5 create_fd5.c)
target_link_libraries(create_fd5 fd5 m)
