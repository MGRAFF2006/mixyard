//! Paired-peer transport and a PCM timeline boundary; deliberately no audio devices or mixer.
#[cfg(test)]
extern crate self as mixyard_network;
pub mod pcm;
pub mod playout;
pub mod protocol;
pub mod timing;
pub mod transport;

pub use pcm::{Counters, Receiver};
pub use protocol::{Codec, Frame, Limits, Offer, Permissions, Provenance, Source, Spec, VERSION};
pub use transport::{Identity, PairedPeer, Session, endpoint};

pub use playout::{
    Playout, PlayoutConfig, PlayoutCounters, PlayoutError, PlayoutReport, PlayoutSnapshot,
    Revocation, RevocationStatus,
};
