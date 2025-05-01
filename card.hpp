//stored in .card files

#include <fstream>
using std::ofstream;
using std::ifsteam;

#include <iostream>
using std::cerr;
using std::endl;

#include "paragraph.hpp"

class card{
  public:
    card(){ //defualt constructor
      length=0;
      readed_length=0;
    }
    card(string filename); // from .card file
    card(paragraph tag, paragraph cite, paragraph metadata, paragraph text); //from components

    int get_length(){ return length; }
    void save_to_file(); // save contents to .card file
    void add_identity(string to_add){
      if(to_add="U"){
        identities[0]=true;
      }else if(to_add="L"){
        identities[1]=true;
      }else if(to_add="!"){
        identities[2]=true;
      }else if(to_add="LD"){
        identities[3]=true;
      }else if(to_add="!D"){
        identities[4]=true;
      }else if(to_add="FW"){
        identities[5]=true;
      }else if(to_add="S"){
        identities[6]=true;
      }else{
        cerr << "tried to add improper identity" << endl;
      }
    }
    void remove_identity(string to_remove){
      if(to_remove="U"){
        identities[0]=false;
      }else if(to_remove="L"){
        identities[1]=false;
      }else if(to_remove="!"){
        identities[2]=false;
      }else if(to_remove="LD"){
        identities[3]=false;
      }else if(to_remove="!D"){
        identities[4]=false;
      }else if(to_remove="FW"){
        identities[5]=false;
      }else if(to_remove="S"){
        identities[6]=false;
      }else{
        cerr << "tried to remove improper identity" << endl;
      }
    }
    
    void set_tag(paragraph tag_object){tag = tag_object; }
    void set_cite(paragraph cite_object){cite = cite_object; }
    void set_metadata(paragraph metadata_object){metadata = metadata_object; }
    void set_text(paragraph text_object){ text = text_object; }
 
    paragraph& get_tag(){ return &tag; }
    paragraph& get_cite(){ return &cite; }
    paragraph& get_metadata(){ return &metadata; }
    paragraph& get_text(){ return &text; }
    
  private:
    bool identities[7]; // U, L, !, LD, !D, FW, S
    long long unsigned int length;
    long long unsigned int readed_length; //amount of words highlighted, set on init and edit
    paragraph tag;
    paragraph cite;
    paragraph metadata;
    paragraph text;
}
