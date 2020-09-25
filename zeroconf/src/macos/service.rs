use super::compat;
use super::service_ref::{ManagedDNSServiceRef, RegisterServiceParams};
use crate::builder::BuilderDelegate;
use crate::ffi::{cstr, FromRaw};
use crate::{NetworkInterface, Result, ServiceRegisteredCallback, ServiceRegistration};
use bonjour_sys::{DNSServiceErrorType, DNSServiceFlags, DNSServiceRef};
use libc::{c_char, c_void};
use std::any::Any;
use std::ffi::CString;
use std::ptr;
use std::sync::Arc;

const BONJOUR_IF_UNSPEC: u32 = 0;
const BONJOUR_RENAME_FLAGS: DNSServiceFlags = 0;

/// Interface for interacting with Bonjour's mDNS service registration capabilities.
#[derive(Debug)]
pub struct BonjourMdnsService {
    service: ManagedDNSServiceRef,
    kind: CString,
    port: u16,
    name: Option<CString>,
    interface_index: u32,
    context: *mut BonjourServiceContext,
}

impl BonjourMdnsService {
    /// Creates a new `BonjourMdnsService` with the specified `kind` (e.g. `_http._tcp`) and
    /// `port`.
    pub fn new(kind: &str, port: u16) -> Self {
        Self {
            service: ManagedDNSServiceRef::default(),
            kind: c_string!(kind),
            port,
            name: None,
            interface_index: BONJOUR_IF_UNSPEC,
            context: Box::into_raw(Box::default()),
        }
    }

    /// Sets the name to register this service under. If no name is set, Bonjour will
    /// automatically assign one (usually to the name of the machine).
    pub fn set_name(&mut self, name: &str) {
        self.name = Some(c_string!(name));
    }

    /// Sets the network interface to bind this service to.
    ///
    /// Most applications will want to use the default value `NetworkInterface::Unspec` to bind to
    /// all available interfaces.
    pub fn set_network_interface(&mut self, interface: NetworkInterface) {
        self.interface_index = match interface {
            NetworkInterface::Unspec => BONJOUR_IF_UNSPEC,
            NetworkInterface::AtIndex(i) => i,
        };
    }

    /// Sets the [`ServiceRegisteredCallback`] that is invoked when the service has been
    /// registered.
    ///
    /// [`ServiceRegisteredCallback`]: ../type.ServiceRegisteredCallback.html
    pub fn set_registered_callback(&mut self, registered_callback: Box<ServiceRegisteredCallback>) {
        unsafe { (*self.context).registered_callback = Some(registered_callback) };
    }

    /// Sets the optional user context to pass through to the callback. This is useful if you need
    /// to share state between pre and post-callback. The context type must implement `Any`.
    pub fn set_context(&mut self, context: Box<dyn Any>) {
        unsafe { (*self.context).user_context = Some(Arc::from(context)) };
    }

    /// Registers and start's the service; continuously polling the event loop. This call will
    /// block the current thread.
    pub fn start(&mut self) -> Result<()> {
        debug!("Registering service: {:?}", self);

        let name = self
            .name
            .as_ref()
            .map(|s| s.as_ptr() as *const c_char)
            .unwrap_or_else(|| ptr::null() as *const c_char);

        self.service.register_service(
            RegisterServiceParams::builder()
                .flags(BONJOUR_RENAME_FLAGS)
                .interface_index(self.interface_index)
                .name(name)
                .regtype(self.kind.as_ptr())
                .domain(ptr::null())
                .host(ptr::null())
                .port(self.port)
                .txt_len(0)
                .txt_record(ptr::null())
                .callback(Some(register_callback))
                .context(self.context as *mut c_void)
                .build()?,
        )
    }
}

impl Drop for BonjourMdnsService {
    fn drop(&mut self) {
        unsafe { Box::from_raw(self.context) };
    }
}

#[derive(Default, FromRaw)]
struct BonjourServiceContext {
    registered_callback: Option<Box<ServiceRegisteredCallback>>,
    user_context: Option<Arc<dyn Any>>,
}

impl BonjourServiceContext {
    fn invoke_callback(&self, result: Result<ServiceRegistration>) {
        if let Some(f) = &self.registered_callback {
            f(result, self.user_context.clone());
        } else {
            warn!("attempted to invoke callback but none was set");
        }
    }
}

unsafe extern "C" fn register_callback(
    _sd_ref: DNSServiceRef,
    _flags: DNSServiceFlags,
    error: DNSServiceErrorType,
    name: *const c_char,
    regtype: *const c_char,
    domain: *const c_char,
    context: *mut c_void,
) {
    let context = BonjourServiceContext::from_raw(context);
    if let Err(e) = handle_register(context, error, domain, name, regtype) {
        context.invoke_callback(Err(e));
    }
}

unsafe fn handle_register(
    context: &BonjourServiceContext,
    error: DNSServiceErrorType,
    domain: *const c_char,
    name: *const c_char,
    regtype: *const c_char,
) -> Result<()> {
    if error != 0 {
        return Err(format!("register_callback() reported error (code: {0})", error).into());
    }

    let domain = compat::normalize_domain(cstr::raw_to_str(domain));

    let result = ServiceRegistration::builder()
        .name(cstr::copy_raw(name))
        .kind(cstr::copy_raw(regtype))
        .domain(domain)
        .build()
        .expect("could not build ServiceRegistration");

    context.invoke_callback(Ok(result));

    Ok(())
}
