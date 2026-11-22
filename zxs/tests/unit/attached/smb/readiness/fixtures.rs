use super::{ERROR,Events,HANG,INVALID,PEER_END,READ};

impl Events{
    pub(in crate::smb) const NONE:Self=Self(0);
    pub(in crate::smb) const READABLE:Self=Self(READ);
    pub(in crate::smb) const REMOTE_END:Self=Self(PEER_END);
    pub(in crate::smb) const HUNG_UP:Self=Self(HANG);
    pub(in crate::smb) const KERNEL_ERROR:Self=Self(ERROR);
    pub(in crate::smb) const UNAVAILABLE:Self=Self(INVALID);
    pub(in crate::smb) const fn with(self,also:Self)->Self{Self(self.0|also.0)}
}
