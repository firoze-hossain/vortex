//! ffi.rs - FFI bindings to NovaOS ring-3 system calls (VFS, DNS, TCP, UDP).

#![allow(dead_code)]

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct NovaUdpAddr {
    pub ip: u32,
    pub port: u16,
}

extern "C" {
    pub fn sys_write(s: *const u8) -> i32;
    pub fn sys_exit(code: i32) -> !;
    pub fn sys_yield() -> i32;

    // VFS
    pub fn sys_open(filename: *const u8) -> i32;
    pub fn sys_read(handle: i32, buf: *mut u8, max_len: i32) -> i32;
    pub fn sys_close(handle: i32);
    pub fn sys_list_files(buf: *mut u8, buf_size: i32) -> i32;
    pub fn sys_write_file(filename: *const u8, data: *const u8, size: u32) -> i32;
    pub fn sys_delete_file(filename: *const u8) -> i32;

    // DNS & TCP/UDP Sockets
    pub fn sys_dns_resolve(hostname: *const u8, out_ip: *mut u32) -> i32;
    pub fn sys_socket() -> i32;
    pub fn sys_connect(handle: i32, dest_ip: u32, dest_port: u16) -> i32;
    pub fn sys_write_handle(handle: i32, buf: *const u8, len: i32) -> i32;
    pub fn sys_socket_udp() -> i32;
    pub fn sys_sendto(handle: i32, addr: *const NovaUdpAddr, buf: *const u8, len: u32) -> i32;
    pub fn sys_recvfrom(
        handle: i32,
        buf: *mut u8,
        max_len: u32,
        out_addr: *mut NovaUdpAddr,
    ) -> i32;
}
